export class ApiError extends Error {
  readonly code: string;
  readonly status?: number;

  constructor(message: string, code = "client_error", status?: number) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
  }
}
