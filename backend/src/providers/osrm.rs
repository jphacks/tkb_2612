//! OSRMの経路アダプタ（behavior.md 5章「OSRM呼び出し」）。
//!
//! `{BASE_URL}/route/v1/{profile}/{lon,lat;…}?alternatives=true&geometries=geojson&overview=full&steps=false`
//! を、上流ゲートを通してから1回だけ送る。値の範囲・頂点数・距離の検証は②が行う。

use std::time::Duration;

use serde_json::Value;

use crate::contract::routing::MAX_ROUTES_PER_CALL;
use crate::contract::{
    BoxFuture, CancelToken, LatLon, LonLat, MalformedRoute, Mode, ProviderRoute, RoutedPath,
    Router, RoutingError, RoutingRequest,
};
use crate::providers::gate::{Gate, GateError};
use crate::providers::http::{FetchError, UpstreamClient};

/// 移動手段ごとの接続先。徒歩・自転車を自動車で代替しない。
#[derive(Debug, Clone)]
pub struct OsrmEndpoints {
    pub walk: reqwest::Url,
    pub bike: reqwest::Url,
    pub car: reqwest::Url,
}

#[derive(Debug, Clone)]
pub struct OsrmRouter {
    endpoints: OsrmEndpoints,
    client: UpstreamClient,
    gate: Gate,
    timeout: Duration,
    max_bytes: usize,
}

impl OsrmRouter {
    pub fn new(
        endpoints: OsrmEndpoints,
        client: UpstreamClient,
        gate: Gate,
        timeout: Duration,
        max_bytes: usize,
    ) -> Self {
        Self {
            endpoints,
            client,
            gate,
            timeout,
            max_bytes,
        }
    }

    fn url(&self, request: &RoutingRequest) -> reqwest::Url {
        let (base, profile) = match request.mode {
            Mode::Walk => (&self.endpoints.walk, "walking"),
            Mode::Bike => (&self.endpoints.bike, "cycling"),
            Mode::Car => (&self.endpoints.car, "driving"),
        };
        route_url(base, profile, request)
    }
}

/// 要求のURL。点はorigin→任意via→destで、各点は `lon小数7桁,lat小数7桁`。
pub fn route_url(base: &reqwest::Url, profile: &str, request: &RoutingRequest) -> reqwest::Url {
    let point = |p: LatLon| format!("{:.7},{:.7}", p.lon, p.lat);
    let points: Vec<String> = std::iter::once(request.origin)
        .chain(request.via)
        .chain(std::iter::once(request.dest))
        .map(point)
        .collect();
    let mut url = base.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .expect("接続先は設定の検査でhttp(s)のURLに限っている");
        segments
            .pop_if_empty()
            .extend(["route", "v1", profile, &points.join(";")]);
    }
    url.set_query(Some(
        "alternatives=true&geometries=geojson&overview=full&steps=false",
    ));
    url
}

impl Router for OsrmRouter {
    fn route<'a>(
        &'a self,
        request: &'a RoutingRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Vec<ProviderRoute>, RoutingError>> {
        Box::pin(async move {
            let lease = self.gate.acquire(cancel).await.map_err(gate_error)?;
            cancel.check()?;
            lease.ensure_valid().map_err(gate_error)?;
            let fetched = self
                .client
                .get(
                    self.url(request),
                    self.timeout.min(lease.remaining()),
                    self.max_bytes,
                    cancel,
                )
                .await;
            lease.release().await;
            let body = fetched.map_err(|e| match e {
                FetchError::Failed => RoutingError::ExternalApi,
                FetchError::Interrupted(i) => RoutingError::Interrupted(i),
            })?;
            parse_response(&body)
        })
    }
}

fn gate_error(e: GateError) -> RoutingError {
    match e {
        GateError::Busy => RoutingError::Busy,
        GateError::Interrupted(i) => RoutingError::Interrupted(i),
    }
}

/// OSRMの応答本文を解釈する。
pub fn parse_response(body: &[u8]) -> Result<Vec<ProviderRoute>, RoutingError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| RoutingError::ExternalApi)?;
    let Value::Object(outer) = value else {
        return Err(RoutingError::NoRoute);
    };
    if outer.get("code").and_then(Value::as_str) != Some("Ok") {
        return Err(RoutingError::NoRoute);
    }
    let Some(Value::Array(routes)) = outer.get("routes") else {
        return Err(RoutingError::ExternalApi);
    };
    if routes.is_empty() {
        return Err(RoutingError::NoRoute);
    }
    let mut parsed = Vec::with_capacity(routes.len().min(MAX_ROUTES_PER_CALL));
    for route in routes.iter().take(MAX_ROUTES_PER_CALL) {
        let one = parse_route(route);
        let malformed = one.is_err();
        parsed.push(one);
        // ②は不正な経路で検査を止めるので、それより後は返さない
        if malformed {
            break;
        }
    }
    Ok(parsed)
}

fn parse_route(route: &Value) -> ProviderRoute {
    let duration_s = route
        .get("duration")
        .and_then(Value::as_f64)
        .ok_or(MalformedRoute)?;
    let distance_m = route
        .get("distance")
        .and_then(Value::as_f64)
        .ok_or(MalformedRoute)?;
    let geometry = route.get("geometry").ok_or(MalformedRoute)?;
    if geometry.get("type").and_then(Value::as_str) != Some("LineString") {
        return Err(MalformedRoute);
    }
    let coordinates = geometry
        .get("coordinates")
        .and_then(Value::as_array)
        .ok_or(MalformedRoute)?
        .iter()
        .map(|p| match p.as_array().map(Vec::as_slice) {
            Some([lon, lat]) => match (lon.as_f64(), lat.as_f64()) {
                (Some(lon), Some(lat)) => Ok(LonLat::new(lon, lat)),
                _ => Err(MalformedRoute),
            },
            _ => Err(MalformedRoute),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RoutedPath {
        duration_s,
        distance_m,
        coordinates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_uses_profile_seven_decimals_and_fixed_query() {
        let base = reqwest::Url::parse("http://127.0.0.1:9/routed-foot/").unwrap();
        let request = RoutingRequest {
            origin: LatLon::new(35.6812, 139.7671),
            dest: LatLon::new(35.7138, 139.7773),
            mode: Mode::Walk,
            via: Some(LatLon::new(35.7, 139.77)),
        };
        assert_eq!(
            route_url(&base, "walking", &request).as_str(),
            "http://127.0.0.1:9/routed-foot/route/v1/walking/\
             139.7671000,35.6812000;139.7700000,35.7000000;139.7773000,35.7138000\
             ?alternatives=true&geometries=geojson&overview=full&steps=false"
        );
    }

    #[test]
    fn outer_shape_maps_to_no_route_or_external_api() {
        assert_eq!(parse_response(b"not json"), Err(RoutingError::ExternalApi));
        assert_eq!(parse_response(b"[]"), Err(RoutingError::NoRoute));
        assert_eq!(
            parse_response(br#"{"code":"NoRoute","routes":[]}"#),
            Err(RoutingError::NoRoute)
        );
        assert_eq!(
            parse_response(br#"{"code":"Ok","routes":[]}"#),
            Err(RoutingError::NoRoute)
        );
        assert_eq!(
            parse_response(br#"{"code":"Ok","routes":{}}"#),
            Err(RoutingError::ExternalApi)
        );
    }

    #[test]
    fn malformed_route_is_placed_at_its_position() {
        let body = br#"{"code":"Ok","routes":[
            {"duration":1,"distance":2,"geometry":{"type":"LineString","coordinates":[[139,35],[139.1,35]]}},
            {"duration":1,"distance":2,"geometry":{"type":"LineString","coordinates":[[139,35,0],[139.1,35]]}},
            {"duration":1,"distance":2,"geometry":{"type":"LineString","coordinates":[[139,35],[139.1,35]]}}
        ]}"#;
        let routes = parse_response(body).unwrap();
        assert_eq!(routes.len(), 2);
        assert!(routes[0].is_ok());
        assert_eq!(routes[1], Err(MalformedRoute));
    }
}
