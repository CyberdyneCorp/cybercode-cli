// Tool schemas: a JSON Schema object, or a Zod-style schema (anything with `safeParse`,
// such as Zod 4) whose JSON Schema comes from its `toJSONSchema()` or an explicit
// `jsonSchema`.

/** A JSON Schema document for an object input. */
export type JsonSchema = Record<string, unknown>;

/** The parsing surface of Zod and compatible libraries. */
export interface SchemaLike<T> {
  safeParse(value: unknown): { success: true; data: T } | { success: false; error: unknown };
  toJSONSchema?(): JsonSchema;
}

/** A tool input or output schema. */
export type Schema<T = unknown> = JsonSchema | SchemaLike<T>;

export function isSchemaLike<T>(schema: unknown): schema is SchemaLike<T> {
  return typeof schema === "object" && schema !== null && typeof (schema as { safeParse?: unknown }).safeParse === "function";
}

/** The JSON Schema the core advertises for `schema`. */
export function toJsonSchema(schema: Schema, explicit?: JsonSchema): JsonSchema {
  if (explicit) return explicit;
  if (!isSchemaLike(schema)) return schema;
  if (typeof schema.toJSONSchema === "function") {
    const { $schema: _ignored, ...rest } = schema.toJSONSchema();
    return rest;
  }
  throw new TypeError("this schema has no toJSONSchema(); pass jsonSchema alongside it");
}

/** Parse `value` with a Zod-style schema; a JSON Schema input is already validated by the core. */
export function parse<T>(schema: Schema<T> | undefined, value: unknown, what: string): T {
  if (!schema || !isSchemaLike<T>(schema)) return value as T;
  const parsed = schema.safeParse(value);
  if (parsed.success) return parsed.data;
  const detail = parsed.error instanceof Error ? parsed.error.message : JSON.stringify(parsed.error);
  throw new Error(`invalid ${what}: ${detail}`);
}
