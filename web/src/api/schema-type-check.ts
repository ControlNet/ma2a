import ts from "typescript"
import {
  assertEnum,
  assertFlag,
  containsFlag,
  exportedType,
  type LocalApiSchema,
  literalValue,
  propertyType,
  requiredNode,
  type SchemaNode,
  SchemaTypeMismatch,
  symbolType,
} from "./schema-type-model"

export function assertGeneratedTypesMatchSchema(schemaJson: string, generatedPath: string): void {
  const schema: LocalApiSchema = JSON.parse(schemaJson)
  const program = ts.createProgram([generatedPath], {
    exactOptionalPropertyTypes: true,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    noUncheckedIndexedAccess: true,
    skipLibCheck: true,
    strict: true,
    target: ts.ScriptTarget.ES2024,
  })
  const checker = program.getTypeChecker()
  const source = program.getSourceFile(generatedPath)
  if (source === undefined) {
    throw new SchemaTypeMismatch(`missing generated source: ${generatedPath}`)
  }
  const moduleSymbol = checker.getSymbolAtLocation(source)
  if (moduleSymbol === undefined) {
    throw new SchemaTypeMismatch("generated source is not a module")
  }
  const exports = new Map(
    checker.getExportsOfModule(moduleSymbol).map((symbol) => [symbol.getName(), symbol]),
  )
  const context = { checker, exports, schema }
  assertNode(
    context,
    exportedType(checker, exports, "SnapshotFragment"),
    requiredNode(schema.types, "snapshot_fragment"),
    "types.snapshot_fragment",
  )
  const successResponse = exportedType(checker, exports, "LocalApiSuccessResponse")
  const errorResponse = exportedType(checker, exports, "LocalApiErrorResponse")
  assertVariants(context, exportedType(checker, exports, "LocalApiCommand"), "operation", {
    entries: schema.commands,
    fixed: { version: { literal: 1 } },
  })
  assertVariants(context, exportedType(checker, exports, "CommandResult"), "type", {
    entries: Object.fromEntries(
      Object.entries(schema.results).map(([name, result]) => [
        name,
        { fields: { payload: result.payload }, required: ["payload"] },
      ]),
    ),
    fixed: {},
  })
  assertNode(
    context,
    successResponse,
    requiredNode(schema.responses, "success"),
    "responses.success",
  )
  assertNode(context, errorResponse, requiredNode(schema.responses, "error"), "responses.error")
  assertExactUnion(
    exportedType(checker, exports, "LocalApiResponse"),
    [successResponse, errorResponse],
    "LocalApiResponse",
  )
  assertVariants(context, exportedType(checker, exports, "RuntimeEvent"), "type", {
    entries: Object.fromEntries(
      Object.entries(schema.events).map(([name, event]) => [
        name,
        {
          fields: { changed: requiredNode(event.fields, "changed"), revision: { ref: "revision" } },
          required: ["changed", "revision"],
        },
      ]),
    ),
    fixed: {},
  })
}

type Context = {
  readonly checker: ts.TypeChecker
  readonly exports: ReadonlyMap<string, ts.Symbol>
  readonly schema: LocalApiSchema
}
type VariantSet = {
  readonly entries: Readonly<Record<string, SchemaNode>>
  readonly fixed: Readonly<Record<string, SchemaNode>>
}

function assertVariants(
  context: Context,
  actual: ts.Type,
  discriminator: string,
  expected: VariantSet,
): void {
  const variants = actual.isUnion() ? actual.types : [actual]
  if (variants.length !== Object.keys(expected.entries).length) {
    throw new SchemaTypeMismatch(`${discriminator}: variant count mismatch`)
  }
  for (const [name, entry] of Object.entries(expected.entries)) {
    const variant = variants.find(
      (candidate) =>
        literalValue(context.checker, propertyType(context.checker, candidate, discriminator)) ===
        name,
    )
    if (variant === undefined) {
      throw new SchemaTypeMismatch(`${discriminator}: missing variant ${name}`)
    }
    const fields = { ...expected.fixed, [discriminator]: { literal: name }, ...entry.fields }
    assertObject(
      context,
      variant,
      { fields, required: Object.keys(fields) },
      `${discriminator}.${name}`,
    )
  }
}

function assertNode(context: Context, actual: ts.Type, expected: SchemaNode, path: string): void {
  if (expected.ref !== undefined) {
    assertReferenceIdentity(context, actual, expected.ref, path)
    assertNode(context, actual, requiredNode(context.schema.types, expected.ref), path)
    return
  }
  if (expected.nullable !== undefined) {
    const members = actual.isUnion() ? actual.types : [actual]
    const valueMembers = members.filter((member) => (member.flags & ts.TypeFlags.Null) === 0)
    if (members.length !== 2 || valueMembers.length !== 1) {
      throw new SchemaTypeMismatch(`${path}: expected nullable type`)
    }
    const value = valueMembers[0]
    if (value === undefined) {
      throw new SchemaTypeMismatch(`${path}: missing non-null type`)
    }
    assertNode(context, value, expected.nullable, path)
    return
  }
  if (expected.literal !== undefined) {
    if (literalValue(context.checker, actual) !== expected.literal) {
      throw new SchemaTypeMismatch(`${path}: literal mismatch`)
    }
    return
  }
  if (expected.one_of === "results") {
    assertVariants(context, actual, "type", {
      entries: Object.fromEntries(
        Object.entries(context.schema.results).map(([name, result]) => [
          name,
          { fields: { payload: result.payload }, required: ["payload"] },
        ]),
      ),
      fixed: {},
    })
    return
  }
  switch (expected.type) {
    case "array": {
      const symbolName = actual.getSymbol()?.getName()
      if (
        !context.checker.isArrayType(actual) ||
        (symbolName !== "Array" && symbolName !== "ReadonlyArray")
      ) {
        throw new SchemaTypeMismatch(`${path}: expected array`)
      }
      const item = context.checker.getIndexTypeOfType(actual, ts.IndexKind.Number)
      if (item === undefined || expected.items === undefined) {
        throw new SchemaTypeMismatch(`${path}: expected array`)
      }
      assertNode(context, item, expected.items, `${path}[]`)
      return
    }
    case "boolean":
      assertFlag(actual, ts.TypeFlags.BooleanLike, path, "boolean")
      return
    case "enum":
      assertEnum(context.checker, actual, expected.values ?? [], path)
      return
    case "integer":
      assertFlag(actual, ts.TypeFlags.NumberLike, path, "number")
      return
    case "object":
      assertObject(context, actual, expected, path)
      return
    case "string":
      if (!containsFlag(actual, ts.TypeFlags.StringLike)) {
        throw new SchemaTypeMismatch(`${path}: expected string`)
      }
      return
    default:
      assertObject(context, actual, expected, path)
  }
}

function assertReferenceIdentity(
  context: Context,
  actual: ts.Type,
  reference: string,
  path: string,
): void {
  const exportName = reference
    .split("_")
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`)
    .join("")
  const expectedSymbol = context.exports.get(exportName)
  if (expectedSymbol === undefined) {
    return
  }
  const expectedType = context.checker.getDeclaredTypeOfSymbol(expectedSymbol)
  if (
    expectedType.isIntersection() &&
    containsFlag(expectedType, ts.TypeFlags.StringLike) &&
    actual.aliasSymbol !== expectedType.aliasSymbol
  ) {
    throw new SchemaTypeMismatch(`${path}: reference identity mismatch`)
  }
}

function assertExactUnion(actual: ts.Type, expected: readonly ts.Type[], path: string): void {
  if (
    !actual.isUnion() ||
    actual.types.length !== expected.length ||
    expected.some((member) =>
      actual.types.every((candidate) => candidate.aliasSymbol !== member.aliasSymbol),
    )
  ) {
    throw new SchemaTypeMismatch(`${path}: union identity mismatch`)
  }
}

function assertObject(context: Context, actual: ts.Type, expected: SchemaNode, path: string): void {
  const fields = expected.fields ?? {}
  const required = new Set(expected.required ?? Object.keys(fields))
  const properties = context.checker.getPropertiesOfType(actual)
  if (properties.length !== Object.keys(fields).length) {
    throw new SchemaTypeMismatch(`${path}: field count mismatch`)
  }
  for (const [name, field] of Object.entries(fields)) {
    const property = properties.find((candidate) => candidate.getName() === name)
    if (property === undefined) {
      throw new SchemaTypeMismatch(`${path}: missing field ${name}`)
    }
    const optional = (property.flags & ts.SymbolFlags.Optional) !== 0
    if (optional === required.has(name)) {
      throw new SchemaTypeMismatch(`${path}.${name}: requiredness mismatch`)
    }
    assertNode(context, symbolType(context.checker, property), field, `${path}.${name}`)
  }
}
