import { createHash } from "node:crypto"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  ERROR_CODES,
  LOCAL_API_SCHEMA_JSON,
  LOCAL_API_SCHEMA_SHA256,
  LOCAL_API_VERSION,
} from "./generated"
import { assertGeneratedTypesMatchSchema } from "./schema-type-check"
import { SchemaTypeMismatch } from "./schema-type-model"

type ContractMutation = {
  readonly file: "generated-models.ts" | "generated.ts"
  readonly original: string
  readonly replacement: string
}

function expectContractMutationRejected(mutation: ContractMutation): void {
  const directory = mkdtempSync(join(tmpdir(), "ma2a-schema-type-"))
  try {
    for (const file of ["generated.ts", "generated-models.ts"] as const) {
      const source = readFileSync(`${import.meta.dir}/${file}`, "utf8")
      const mutated =
        file === mutation.file ? source.replace(mutation.original, mutation.replacement) : source
      expect(file !== mutation.file || mutated !== source).toBe(true)
      writeFileSync(join(directory, file), mutated)
    }
    expect(() =>
      assertGeneratedTypesMatchSchema(LOCAL_API_SCHEMA_JSON, join(directory, "generated.ts")),
    ).toThrow(SchemaTypeMismatch)
  } finally {
    rmSync(directory, { recursive: true })
  }
}

test("generated local API contract matches the independent golden hash", () => {
  const digest = createHash("sha256").update(LOCAL_API_SCHEMA_JSON).digest("hex")

  expect(LOCAL_API_VERSION).toBe(1)
  expect(ERROR_CODES).toHaveLength(9)
  expect(digest).toBe(LOCAL_API_SCHEMA_SHA256)
  expect(JSON.parse(LOCAL_API_SCHEMA_JSON).privacy.excluded).toContain("authorized_via")
})

test("machine schema describes nested commands, envelopes, results, and events", () => {
  const schema = JSON.parse(LOCAL_API_SCHEMA_JSON)

  expect(schema.types.runtime_snapshot.fields.private_relay_candidates.items.ref).toBe(
    "private_relay_candidate",
  )
  expect(schema.types.runtime_snapshot.fields.public_relay_fallbacks.items.ref).toBe(
    "public_relay_fallback",
  )
  expect(schema.types.public_relay_fallback.fields.provider_endpoint_id).toBeUndefined()
  expect(schema.types.reachability.fields.state.values).toContain("IrohHomeConnected")
  expect(schema.commands.space_invite.fields.ttl_ms.maximum).toBe(300_000)
  expect(schema.commands.space_invite.fields.output_path.ref).toBe("path")
  expect(schema.commands.private_relay_configure.fields.served_space_ids.items.ref).toBe("space_id")
  expect(schema.commands.private_relay_disable.fields.request_id.ref).toBe("request_id")
  expect(schema.commands.public_relay_disable.fields.request_id.ref).toBe("request_id")
  expect(schema.commands.ui_status.fields).toEqual({})
  expect(schema.responses.success.fields.request_id.nullable.ref).toBe("request_id")
  expect(schema.responses.error.fields.error.values).toEqual(ERROR_CODES)
  expect(schema.results.snapshot.payload.ref).toBe("runtime_snapshot")
  expect(schema.results.ui_status.payload.ref).toBe("ui_status")
  expect(schema.events.spaces_changed.fields.changed.ref).toBe("changed_space_ids")
})

test("generated TypeScript wire types recursively match the machine schema", () => {
  assertGeneratedTypesMatchSchema(LOCAL_API_SCHEMA_JSON, `${import.meta.dir}/generated.ts`)
})

test("rejects a cross-brand array item when the schema references space_id", () => {
  expectContractMutationRejected({
    file: "generated.ts",
    original: "readonly changed: { readonly space_ids: readonly SpaceId[] }",
    replacement: "readonly changed: { readonly space_ids: readonly EndpointId[] }",
  })
})

test("rejects a branded scalar when the schema requires an array", () => {
  expectContractMutationRejected({
    file: "generated.ts",
    original: "readonly changed: { readonly space_ids: readonly SpaceId[] }",
    replacement: "readonly changed: { readonly space_ids: SpaceId }",
  })
})

test("rejects a cross-brand scalar when the schema references endpoint_id", () => {
  expectContractMutationRejected({
    file: "generated-models.ts",
    original: "readonly endpoint_id: EndpointId",
    replacement: "readonly endpoint_id: SpaceId",
  })
})

test("rejects an unrelated exported LocalApiResponse type", () => {
  expectContractMutationRejected({
    file: "generated.ts",
    original: "export type LocalApiResponse = LocalApiSuccessResponse | LocalApiErrorResponse",
    replacement: "export type LocalApiResponse = string",
  })
})
