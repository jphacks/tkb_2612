//! 右左折禁止（形式書9章）。「続けて通ってはいけない有向辺の列」に統一して持つ。
use serde_json::Value;

use crate::contract::Mode;

use crate::domain::routing::graph::{DirEdge, Graph};

/// 禁止の1件（`RESTR` 節のレコードを `SEQ` で展開したもの）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restriction {
    /// 禁止する有向辺の列（長さ2以上）。隣り合う有向辺がつながっている。
    pub seq: Vec<DirEdge>,
    /// 対象の手段のビット（walk=1、bike=2、car=4。`Mode::mask`）。0にならない。
    pub modes: u8,
    /// 元のrelation（0：via-node、1：via-way）。
    pub kind: u8,
}

impl Restriction {
    /// `RESTR` 節の `elem_size`（`{seq_start: u32, seq_len: u16, modes: u8, kind: u8}`）。
    pub const RECORD_SIZE: usize = 8;
}

/// `restrictions.bin` の中身。
#[derive(Debug, Clone, PartialEq)]
pub struct Restrictions {
    /// `PARAMS`（JSON）。
    pub params: Value,
    /// 列の辞書順（`SEQ` の値の並び）で並べ、同じ列・同じ手段は1つにまとめる。
    pub items: Vec<Restriction>,
}

impl Restrictions {
    /// 禁止がない（`restrictions.bin` が空）場合。合成グラフのテストにも使う。
    pub fn empty() -> Self {
        Self {
            params: Value::Object(Default::default()),
            items: Vec::new(),
        }
    }

    /// 読込時の検査（形式書9章）：列の有向辺がすべて2E未満、隣り合う有向辺がつながっていること、
    /// `modes ≠ 0`、`seq_len ≥ 2`、並び順。
    ///
    /// 次も確かめる。`modes` は walk・bike・car のビット（1・2・4）だけ。`kind` は0か1。
    /// 並びは列の辞書順（`DirEdge` の値の順）で、同じ列が続くときは手段のビットが重ならず `modes` の昇順
    /// （同じ列・同じ手段は1つにまとめる、という規則。同じ列を手段ごとに分けて持つ形も許す）。
    /// 先頭から調べ、最初に見つかった違反の件の添字を返す。
    pub fn validate(&self, graph: &Graph) -> Result<(), RestrictionError> {
        const MODE_BITS: u8 = 0b111;
        let dedge_limit = graph.edge_count() as u64 * 2;
        let mut prev: Option<&Restriction> = None;
        for (item, r) in self.items.iter().enumerate() {
            if r.seq.len() < 2 || r.modes == 0 || r.modes & !MODE_BITS != 0 || r.kind > 1 {
                return Err(RestrictionError::Malformed { item });
            }
            if r.seq.iter().any(|d| u64::from(d.0) >= dedge_limit) {
                return Err(RestrictionError::DirEdgeOutOfRange { item });
            }
            if r.seq
                .windows(2)
                .any(|w| graph.dedge_to(w[0]) != graph.dedge_from(w[1]))
            {
                return Err(RestrictionError::NotConnected { item });
            }
            if let Some(p) = prev {
                let ordered = match p.seq.cmp(&r.seq) {
                    std::cmp::Ordering::Less => true,
                    std::cmp::Ordering::Equal => p.modes & r.modes == 0 && p.modes < r.modes,
                    std::cmp::Ordering::Greater => false,
                };
                if !ordered {
                    return Err(RestrictionError::NotSorted { item });
                }
            }
            prev = Some(r);
        }
        Ok(())
    }
}

/// `Restrictions::validate` の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionError {
    /// 有向辺の番号が2E以上。
    DirEdgeOutOfRange { item: usize },
    /// 隣り合う有向辺がつながっていない。
    NotConnected { item: usize },
    /// 列が2未満、または `modes` が0。
    Malformed { item: usize },
    /// 並び順・重複の違反。
    NotSorted { item: usize },
}

impl std::fmt::Display for RestrictionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestrictionError::DirEdgeOutOfRange { item } => {
                write!(f, "restrictions.binの{item}件目の有向辺が範囲外です")
            }
            RestrictionError::NotConnected { item } => {
                write!(f, "restrictions.binの{item}件目の列がつながっていません")
            }
            RestrictionError::Malformed { item } => {
                write!(f, "restrictions.binの{item}件目の列または手段が不正です")
            }
            RestrictionError::NotSorted { item } => {
                write!(
                    f,
                    "restrictions.binの{item}件目が並び順どおりではありません"
                )
            }
        }
    }
}

impl std::error::Error for RestrictionError {}

/// 手段ごとのAho–Corasick照合器（形式書9章）。探索の状態は「直前の有向辺＋照合器の状態」で、
/// 禁止の列の末尾に達する遷移は通れない。状態数は、列の接頭辞の数＋1以下。
///
/// 文字は `DirEdge` の値。状態はトライの節で、番号は深さの順（同じ深さでは列の辞書順）に振る。
/// こうすると、ある状態の子の番号が連続し、遷移の記号の昇順に並ぶので、遷移は
/// `symbol[child_ofs[s]..child_ofs[s+1]]` の二分探索で引ける（状態ごとのハッシュ表は持たない）。
/// 遷移がなければ失敗遷移 `fail` をたどる。禁止の列そのもの、またはその接尾辞が禁止の列である状態は
/// 「行き止まり」（`dead`）で、そこへの遷移は `None`。
#[derive(Debug, Clone)]
pub struct TurnAutomaton {
    /// 状態数（根を含む）。
    states: u32,
    /// 状態sの子の番号の範囲は `child_ofs[s]..child_ofs[s+1]`。長さ `states + 1`。
    child_ofs: Vec<u32>,
    /// 各状態へ入る遷移の記号（`DirEdge::0`）。根は0（使わない）。長さ `states`。
    symbol: Vec<u32>,
    /// 失敗遷移。長さ `states`。
    fail: Vec<u32>,
    /// 行き止まりの状態か。長さ `states`。
    dead: Vec<bool>,
}

impl TurnAutomaton {
    /// その手段（`modes` に `mode.mask()` を含む）の禁止だけから照合器を作る。
    ///
    /// 列を辞書順に並べ、ほかの列を接頭辞に持つ列（冗長）と重複を除いてから、深さごとにトライを作る
    /// （`Restrictions` の並びに依存しない）。長さ0の列は無視する。
    pub fn build(restrictions: &Restrictions, mode: Mode) -> TurnAutomaton {
        let mask = mode.mask();
        let mut sorted: Vec<&[DirEdge]> = restrictions
            .items
            .iter()
            .filter(|r| r.modes & mask != 0 && !r.seq.is_empty())
            .map(|r| r.seq.as_slice())
            .collect();
        sorted.sort_unstable();
        sorted.dedup();
        // 辞書順では、ある列を接頭辞に持つ列はその直後に続く。直前に残した列だけを調べればよい。
        let mut patterns: Vec<&[DirEdge]> = Vec::with_capacity(sorted.len());
        for p in sorted {
            if patterns.last().is_some_and(|last| p.starts_with(last)) {
                continue;
            }
            patterns.push(p);
        }

        let mut child_ofs: Vec<u32> = Vec::new();
        let mut symbol: Vec<u32> = vec![0];
        let mut terminal: Vec<bool> = vec![false];
        let mut states: u32 = 1;
        // その深さの節に居る列（列の番号と節の番号）。列の辞書順 = 節の番号順に並ぶ。
        let mut active: Vec<(usize, u32)> = (0..patterns.len()).map(|i| (i, 0)).collect();
        let mut next_node: u32 = 0;
        let mut layer_end: u32 = 1;
        let mut depth = 0;
        while !active.is_empty() {
            let mut next_active = Vec::with_capacity(active.len());
            let mut last_key: Option<(u32, u32)> = None;
            for &(pi, node) in &active {
                let sym = patterns[pi][depth].0;
                while next_node <= node {
                    child_ofs.push(states);
                    next_node += 1;
                }
                let child = if last_key == Some((node, sym)) {
                    states - 1
                } else {
                    symbol.push(sym);
                    terminal.push(false);
                    states = states
                        .checked_add(1)
                        .expect("照合器の状態数がu32に収まりません");
                    last_key = Some((node, sym));
                    states - 1
                };
                if patterns[pi].len() == depth + 1 {
                    terminal[child as usize] = true;
                } else {
                    next_active.push((pi, child));
                }
            }
            while next_node < layer_end {
                child_ofs.push(states);
                next_node += 1;
            }
            layer_end = states;
            active = next_active;
            depth += 1;
        }
        while child_ofs.len() <= states as usize {
            child_ofs.push(states);
        }

        // 失敗遷移と行き止まりの伝播。番号順（深さの順）に、子の失敗遷移を決める。
        let n = states as usize;
        let mut fail = vec![0_u32; n];
        let mut dead = terminal;
        for parent in 0..n {
            for child in child_ofs[parent]..child_ofs[parent + 1] {
                let sym = symbol[child as usize];
                let mut target = 0;
                if parent != 0 {
                    let mut f = fail[parent];
                    target = loop {
                        if let Some(u) = find_child(&child_ofs, &symbol, f, sym) {
                            break u;
                        }
                        if f == 0 {
                            break 0;
                        }
                        f = fail[f as usize];
                    };
                }
                fail[child as usize] = target;
                if dead[target as usize] {
                    dead[child as usize] = true;
                }
            }
        }
        TurnAutomaton {
            states,
            child_ofs,
            symbol,
            fail,
            dead,
        }
    }

    /// 出発直後の状態。
    pub fn start(&self) -> u32 {
        0
    }

    /// 状態数。
    pub fn state_count(&self) -> u32 {
        self.states
    }

    /// 有向辺 `d` を通った後の状態。禁止の列の末尾に達するなら通れないので `None`。
    pub fn step(&self, state: u32, d: DirEdge) -> Option<u32> {
        let mut s = state;
        loop {
            if let Some(next) = find_child(&self.child_ofs, &self.symbol, s, d.0) {
                return (!self.dead[next as usize]).then_some(next);
            }
            if s == 0 {
                return Some(0);
            }
            s = self.fail[s as usize];
        }
    }
}

/// 状態 `state` から記号 `sym` で進む子（トライの辺）。
fn find_child(child_ofs: &[u32], symbol: &[u32], state: u32, sym: u32) -> Option<u32> {
    let lo = child_ofs[state as usize];
    let hi = child_ofs[state as usize + 1];
    symbol[lo as usize..hi as usize]
        .binary_search(&sym)
        .ok()
        .map(|i| lo + i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::synthetic::{ALL_MODES_MASK, Rng, grid};
    use std::collections::{BTreeSet, HashSet};

    fn restriction(seq: &[DirEdge], modes: u8) -> Restriction {
        Restriction {
            seq: seq.to_vec(),
            modes,
            kind: u8::from(seq.len() > 2),
        }
    }

    fn restrictions(items: Vec<Restriction>) -> Restrictions {
        Restrictions {
            items,
            ..Restrictions::empty()
        }
    }

    /// 3×3の格子と、いくつかのつながった有向辺の列。
    struct Fixture {
        graph: Graph,
        /// (0,0)→(0,1)→(0,2)
        a: Vec<DirEdge>,
        /// (0,0)→(0,1)→(1,1)→(2,1)
        b: Vec<DirEdge>,
    }

    fn fixture() -> Fixture {
        let (builder, s) = grid(3, 3, 100.0);
        let n = |r, c| s.node(r, c);
        let a = builder.dedges(&[n(0, 0), n(0, 1), n(0, 2)]);
        let b = builder.dedges(&[n(0, 0), n(0, 1), n(1, 1), n(2, 1)]);
        Fixture {
            graph: builder.build(),
            a,
            b,
        }
    }

    #[test]
    fn validate_accepts_canonical_files() {
        let f = fixture();
        assert_eq!(Restrictions::empty().validate(&f.graph), Ok(()));
        let r = restrictions(vec![
            restriction(&f.a, Mode::Car.mask()),
            restriction(&f.b, ALL_MODES_MASK),
        ]);
        // 辞書順：a は b の接頭辞なので先。
        assert!(f.a < f.b);
        assert_eq!(r.validate(&f.graph), Ok(()));
        // 同じ列を手段ごとに分けて、手段のビットが重ならず昇順なら受け付ける。
        let split = restrictions(vec![
            restriction(&f.a, Mode::Walk.mask()),
            restriction(&f.a, Mode::Car.mask()),
        ]);
        assert_eq!(split.validate(&f.graph), Ok(()));
    }

    #[test]
    fn validate_rejects_each_violation() {
        let f = fixture();
        let g = &f.graph;
        let e = |r: Restrictions| r.validate(g);
        let dedge_limit = g.edge_count() as u32 * 2;

        // 範囲外
        let out_of_range = restriction(&[f.a[0], DirEdge(dedge_limit)], 1);
        assert_eq!(
            e(restrictions(vec![out_of_range])),
            Err(RestrictionError::DirEdgeOutOfRange { item: 0 })
        );
        let huge = restriction(&[DirEdge(u32::MAX), f.a[0]], 1);
        assert_eq!(
            e(restrictions(vec![restriction(&f.a, 1), huge])),
            Err(RestrictionError::DirEdgeOutOfRange { item: 1 })
        );

        // つながっていない（(0,0)→(0,1) の次に (1,0)→(1,1)）
        let (b2, s) = grid(3, 3, 100.0);
        let far = b2.dedge(s.node(1, 0), s.node(1, 1));
        assert_eq!(
            e(restrictions(vec![restriction(&[f.a[0], far], 7)])),
            Err(RestrictionError::NotConnected { item: 0 })
        );
        // 逆向きに戻る列（(0,0)→(0,1)→(0,0)）はつながっている。
        assert_eq!(
            e(restrictions(vec![restriction(
                &[f.a[0], f.a[0].reversed()],
                7
            )])),
            Ok(())
        );
        // 途中だけつながっていない
        assert_eq!(
            e(restrictions(vec![restriction(
                &[f.a[0], f.a[1], f.a[0]],
                7
            )])),
            Err(RestrictionError::NotConnected { item: 0 })
        );

        // 長さ・手段・kind
        for bad in [
            Restriction {
                seq: vec![f.a[0]],
                modes: 7,
                kind: 0,
            },
            Restriction {
                seq: vec![],
                modes: 7,
                kind: 0,
            },
            Restriction {
                seq: f.a.clone(),
                modes: 0,
                kind: 0,
            },
            Restriction {
                seq: f.a.clone(),
                modes: 8,
                kind: 0,
            },
            Restriction {
                seq: f.a.clone(),
                modes: 7,
                kind: 2,
            },
        ] {
            assert_eq!(
                e(restrictions(vec![bad])),
                Err(RestrictionError::Malformed { item: 0 })
            );
        }

        // 並び順：逆順、同じ列と同じ手段の重複、手段が重なる、手段が降順
        assert!(f.a < f.b);
        assert_eq!(
            e(restrictions(vec![
                restriction(&f.b, 7),
                restriction(&f.a, 7)
            ])),
            Err(RestrictionError::NotSorted { item: 1 })
        );
        assert_eq!(
            e(restrictions(vec![
                restriction(&f.a, 4),
                restriction(&f.a, 4)
            ])),
            Err(RestrictionError::NotSorted { item: 1 })
        );
        assert_eq!(
            e(restrictions(vec![
                restriction(&f.a, 6),
                restriction(&f.a, 4)
            ])),
            Err(RestrictionError::NotSorted { item: 1 })
        );
        assert_eq!(
            e(restrictions(vec![
                restriction(&f.a, 4),
                restriction(&f.a, 1)
            ])),
            Err(RestrictionError::NotSorted { item: 1 })
        );
        // 最初に見つかった違反の件を返す。
        assert_eq!(
            e(restrictions(vec![
                restriction(&f.a, 7),
                restriction(&f.a, 7),
                restriction(&[f.a[0], far], 7),
            ])),
            Err(RestrictionError::NotSorted { item: 1 })
        );
    }

    #[test]
    fn restriction_errors_have_readable_messages() {
        let message = RestrictionError::NotConnected { item: 3 }.to_string();
        assert!(message.contains("restrictions.bin") && message.contains('3'));
    }

    // ---- 照合器 ----

    fn d(x: u32) -> DirEdge {
        DirEdge(x)
    }

    fn seq(xs: &[u32]) -> Vec<DirEdge> {
        xs.iter().map(|&x| d(x)).collect()
    }

    fn automaton(patterns: &[(&[u32], u8)], mode: Mode) -> TurnAutomaton {
        let mut items: Vec<Restriction> = patterns
            .iter()
            .map(|&(xs, modes)| restriction(&seq(xs), modes))
            .collect();
        items.sort_by(|a, b| a.seq.cmp(&b.seq));
        TurnAutomaton::build(&restrictions(items), mode)
    }

    /// 経路を通して、最初に通れなくなる位置（添字）。
    fn first_blocked_at(a: &TurnAutomaton, path: &[u32]) -> Option<usize> {
        let mut state = a.start();
        for (i, &x) in path.iter().enumerate() {
            match a.step(state, d(x)) {
                Some(next) => state = next,
                None => return Some(i),
            }
        }
        None
    }

    /// 素朴な部分列照合：禁止の列のどれかが `path[..=j]` の接尾辞になる最初の `j`。
    fn naive_first_violation(patterns: &[Vec<u32>], path: &[u32]) -> Option<usize> {
        (0..path.len()).find(|&j| {
            patterns
                .iter()
                .any(|p| !p.is_empty() && p.len() <= j + 1 && path[j + 1 - p.len()..=j] == p[..])
        })
    }

    #[test]
    fn empty_automaton_allows_everything() {
        let a = TurnAutomaton::build(&Restrictions::empty(), Mode::Car);
        assert_eq!((a.start(), a.state_count()), (0, 1));
        let mut s = a.start();
        for x in [5, 0, 9, 5, 5] {
            s = a.step(s, d(x)).unwrap();
            assert_eq!(s, 0);
        }
    }

    #[test]
    fn via_node_restriction_blocks_only_the_pair() {
        let a = automaton(&[(&[10, 20], ALL_MODES_MASK)], Mode::Car);
        assert_eq!(a.state_count(), 1 + 2);
        assert_eq!(first_blocked_at(&a, &[10, 20]), Some(1));
        assert_eq!(first_blocked_at(&a, &[10, 21, 20]), None);
        assert_eq!(first_blocked_at(&a, &[20, 10]), None);
        assert_eq!(first_blocked_at(&a, &[10, 10, 20]), Some(2));
        assert_eq!(first_blocked_at(&a, &[1, 2, 3, 10, 20, 4]), Some(4));
        assert_eq!(first_blocked_at(&a, &[10]), None);
    }

    #[test]
    fn via_way_restriction_follows_the_whole_sequence() {
        let a = automaton(&[(&[1, 2, 3], ALL_MODES_MASK)], Mode::Bike);
        assert_eq!(a.state_count(), 1 + 3);
        assert_eq!(first_blocked_at(&a, &[1, 2, 3]), Some(2));
        assert_eq!(first_blocked_at(&a, &[1, 2, 4]), None);
        assert_eq!(first_blocked_at(&a, &[2, 3]), None);
        assert_eq!(first_blocked_at(&a, &[1, 1, 2, 3]), Some(3));
        assert_eq!(first_blocked_at(&a, &[9, 1, 2, 3]), Some(3));
    }

    #[test]
    fn failure_transitions_find_a_pattern_starting_inside_another() {
        // 長い列の途中から別の列が始まる：1,2,3,4 の途中の 2,3 から 2,3,5 が始まる。
        let a = automaton(
            &[
                (&[1, 2, 3, 4], ALL_MODES_MASK),
                (&[2, 3, 5], ALL_MODES_MASK),
            ],
            Mode::Car,
        );
        assert_eq!(first_blocked_at(&a, &[1, 2, 3, 4]), Some(3));
        assert_eq!(first_blocked_at(&a, &[1, 2, 3, 5]), Some(3));
        assert_eq!(first_blocked_at(&a, &[2, 3, 5]), Some(2));
        assert_eq!(first_blocked_at(&a, &[2, 3, 4]), None);
        assert_eq!(first_blocked_at(&a, &[1, 2, 2, 3, 5]), Some(4));
        assert_eq!(first_blocked_at(&a, &[1, 2, 3, 3, 5]), None);
        // 列の接尾辞が別の列になる：3,1,2 は 1,2 を含む
        let b = automaton(
            &[(&[1, 2], ALL_MODES_MASK), (&[3, 1, 2, 9], ALL_MODES_MASK)],
            Mode::Car,
        );
        assert_eq!(first_blocked_at(&b, &[3, 1, 2, 9]), Some(2));
        assert_eq!(first_blocked_at(&b, &[3, 1, 9]), None);
    }

    #[test]
    fn automata_are_built_per_mode() {
        let patterns: [(&[u32], u8); 3] = [
            (&[1, 2], Mode::Car.mask()),
            (&[3, 4], Mode::Bike.mask() | Mode::Car.mask()),
            (&[5, 6], Mode::Walk.mask()),
        ];
        let car = automaton(&patterns, Mode::Car);
        let bike = automaton(&patterns, Mode::Bike);
        let walk = automaton(&patterns, Mode::Walk);
        assert_eq!(first_blocked_at(&car, &[1, 2]), Some(1));
        assert_eq!(first_blocked_at(&bike, &[1, 2]), None);
        assert_eq!(first_blocked_at(&walk, &[1, 2]), None);
        assert_eq!(first_blocked_at(&car, &[3, 4]), Some(1));
        assert_eq!(first_blocked_at(&bike, &[3, 4]), Some(1));
        assert_eq!(first_blocked_at(&walk, &[3, 4]), None);
        assert_eq!(first_blocked_at(&walk, &[5, 6]), Some(1));
        assert_eq!(first_blocked_at(&car, &[5, 6]), None);
        assert_eq!(
            (car.state_count(), bike.state_count(), walk.state_count()),
            (5, 3, 3)
        );
    }

    #[test]
    fn redundant_and_duplicate_sequences_do_not_add_states() {
        // [1,2] を含む [1,2,3] は冗長。同じ列の重複も1つ。
        let a = automaton(
            &[(&[1, 2], 7), (&[1, 2, 3], 7), (&[1, 2], 4), (&[4, 5], 7)],
            Mode::Car,
        );
        assert_eq!(a.state_count(), 1 + 2 + 2);
        assert_eq!(first_blocked_at(&a, &[1, 2, 3]), Some(1));
    }

    #[test]
    fn a_length_one_sequence_blocks_the_dedge_itself() {
        let r = restrictions(vec![restriction(&seq(&[7]), 4)]);
        let a = TurnAutomaton::build(&r, Mode::Car);
        assert_eq!(a.step(a.start(), d(7)), None);
        assert!(a.step(a.start(), d(8)).is_some());
    }

    #[test]
    fn state_count_is_at_most_distinct_prefixes_plus_one() {
        let mut rng = Rng::new(11);
        for _ in 0..200 {
            let patterns = random_patterns(&mut rng, 4, 8, 5);
            let items: Vec<Restriction> = patterns
                .iter()
                .map(|p| restriction(&p.iter().map(|&x| d(x)).collect::<Vec<_>>(), 7))
                .collect();
            let a = TurnAutomaton::build(&restrictions(items), Mode::Car);
            let prefixes: BTreeSet<&[u32]> = patterns
                .iter()
                .flat_map(|p| (1..=p.len()).map(move |n| &p[..n]))
                .collect();
            assert!(a.state_count() as usize <= prefixes.len() + 1);
        }
    }

    fn random_patterns(
        rng: &mut Rng,
        alphabet: u32,
        max_count: usize,
        max_len: usize,
    ) -> Vec<Vec<u32>> {
        let count = rng.below(max_count + 1);
        (0..count)
            .map(|_| {
                let len = 1 + rng.below(max_len);
                (0..len)
                    .map(|_| rng.below(alphabet as usize) as u32)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn automaton_matches_naive_substring_matching_on_random_inputs() {
        let mut rng = Rng::new(2024);
        let mut blocked_cases = 0;
        let mut total = 0;
        for case in 0..6000 {
            // 小さな文字集合で、列の重なり・途中からの開始・接尾辞の包含を起こしやすくする。
            let alphabet = 2 + (case % 4) as u32;
            let patterns = random_patterns(&mut rng, alphabet, 8, 5);
            let items: Vec<Restriction> = patterns
                .iter()
                .map(|p| restriction(&p.iter().map(|&x| d(x)).collect::<Vec<_>>(), 7))
                .collect();
            // ファイルの並び順に依存しないことも確かめる（並べ替えた入力でも同じ）。
            let mut shuffled = items.clone();
            rng.shuffle(&mut shuffled);
            let a = TurnAutomaton::build(&restrictions(items), Mode::Car);
            let b = TurnAutomaton::build(&restrictions(shuffled), Mode::Car);
            assert_eq!(a.state_count(), b.state_count());
            for _ in 0..8 {
                let len = rng.below(16);
                let path: Vec<u32> = (0..len)
                    .map(|_| rng.below(alphabet as usize) as u32)
                    .collect();
                let want = naive_first_violation(&patterns, &path);
                assert_eq!(first_blocked_at(&a, &path), want, "{patterns:?} {path:?}");
                assert_eq!(first_blocked_at(&b, &path), want);
                total += 1;
                blocked_cases += usize::from(want.is_some());
            }
        }
        // 禁止にあたる経路とあたらない経路の両方が十分に出ている。
        assert!(
            blocked_cases > total / 10 && blocked_cases < total * 9 / 10,
            "{blocked_cases}/{total}"
        );
    }

    #[test]
    fn automaton_matches_naive_matching_per_mode() {
        let mut rng = Rng::new(77);
        for _ in 0..2000 {
            let alphabet = 3;
            let patterns = random_patterns(&mut rng, alphabet, 6, 4);
            let modes: Vec<u8> = patterns.iter().map(|_| 1 + rng.below(7) as u8).collect();
            let items: Vec<Restriction> = patterns
                .iter()
                .zip(&modes)
                .map(|(p, &m)| restriction(&p.iter().map(|&x| d(x)).collect::<Vec<_>>(), m))
                .collect();
            let mut items = items;
            items.sort_by(|a, b| a.seq.cmp(&b.seq).then(a.modes.cmp(&b.modes)));
            let r = restrictions(items);
            for mode in Mode::ALL {
                let a = TurnAutomaton::build(&r, mode);
                let applicable: Vec<Vec<u32>> = patterns
                    .iter()
                    .zip(&modes)
                    .filter(|(_, m)| **m & mode.mask() != 0)
                    .map(|(p, _)| p.clone())
                    .collect();
                for _ in 0..4 {
                    let path: Vec<u32> = (0..rng.below(14)).map(|_| rng.below(3) as u32).collect();
                    assert_eq!(
                        first_blocked_at(&a, &path),
                        naive_first_violation(&applicable, &path)
                    );
                }
            }
        }
    }

    #[test]
    fn automaton_scales_to_hundreds_of_thousands_of_sequences() {
        let mut rng = Rng::new(5);
        let alphabet = 300;
        let patterns: Vec<Vec<u32>> = (0..200_000)
            .map(|_| {
                let len = 3 + rng.below(4);
                (0..len).map(|_| rng.below(alphabet) as u32).collect()
            })
            .collect();
        let mut items: Vec<Restriction> = patterns
            .iter()
            .map(|p| restriction(&p.iter().map(|&x| d(x)).collect::<Vec<_>>(), 4))
            .collect();
        items.sort_by(|a, b| a.seq.cmp(&b.seq));
        let r = restrictions(items);
        let a = TurnAutomaton::build(&r, Mode::Car);
        let prefixes: HashSet<&[u32]> = patterns
            .iter()
            .flat_map(|p| (1..=p.len()).map(move |n| &p[..n]))
            .collect();
        assert!(a.state_count() as usize <= prefixes.len() + 1);
        assert!(a.state_count() > 100_000);
        // 別の手段には1状態だけ。
        assert_eq!(TurnAutomaton::build(&r, Mode::Walk).state_count(), 1);
        // 禁止の列の断片をつなげた経路を、集合を引く照合と比べる。
        let set: HashSet<&[u32]> = patterns.iter().map(Vec::as_slice).collect();
        for _ in 0..300 {
            let mut path = Vec::new();
            for _ in 0..4 {
                let p = &patterns[rng.below(patterns.len())];
                let from = rng.below(p.len());
                let to = from + 1 + rng.below(p.len() - from);
                path.extend_from_slice(&p[from..to]);
            }
            let want =
                (0..path.len()).find(|&j| (1..=j + 1).any(|n| set.contains(&path[j + 1 - n..=j])));
            assert_eq!(first_blocked_at(&a, &path), want);
        }
    }
}
