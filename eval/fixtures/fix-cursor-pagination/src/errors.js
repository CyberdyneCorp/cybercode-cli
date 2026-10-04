/** Errors raised by the API layer. `code` is stable and meant for clients; `status` is HTTP-like. */
export class ApiError extends Error {
  constructor(code, message, status = 400) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
  }
}

export const invalidParam = (message) => new ApiError("INVALID_PARAM", message);
export const invalidCursor = () => new ApiError("INVALID_CURSOR", "invalid cursor");
export const cursorMismatch = () =>
  new ApiError("CURSOR_MISMATCH", "cursor does not match the current sort and filters");
