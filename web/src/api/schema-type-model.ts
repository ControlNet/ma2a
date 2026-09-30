import ts from "typescript"

export type SchemaNode = {
  readonly fields?: Readonly<Record<string, SchemaNode>>
  readonly items?: SchemaNode
  readonly literal?: number | string
  readonly nullable?: SchemaNode
  readonly one_of?: string
  readonly ref?: string
  readonly required?: readonly string[]
  readonly type?: string
  readonly values?: readonly string[]
}

export type LocalApiSchema = {
  readonly commands: Readonly<Record<string, SchemaNode>>
  readonly events: Readonly<Record<string, SchemaNode>>
  readonly responses: Readonly<Record<string, SchemaNode>>
  readonly results: Readonly<Record<string, { readonly payload: SchemaNode }>>
  readonly types: Readonly<Record<string, SchemaNode>>
}

export class SchemaTypeMismatch extends Error {
  readonly name = "SchemaTypeMismatch"
}

export function exportedType(
  checker: ts.TypeChecker,
  exports: ReadonlyMap<string, ts.Symbol>,
  name: string,
): ts.Type {
  const symbol = exports.get(name)
  if (symbol === undefined) {
    throw new SchemaTypeMismatch(`missing exported type ${name}`)
  }
  return checker.getDeclaredTypeOfSymbol(symbol)
}

export function propertyType(checker: ts.TypeChecker, owner: ts.Type, name: string): ts.Type {
  const property = checker.getPropertyOfType(owner, name)
  if (property === undefined) {
    throw new SchemaTypeMismatch(`missing discriminator ${name}`)
  }
  return symbolType(checker, property)
}

export function symbolType(checker: ts.TypeChecker, symbol: ts.Symbol): ts.Type {
  const declaration = symbol.valueDeclaration ?? symbol.declarations?.at(0)
  if (declaration === undefined) {
    throw new SchemaTypeMismatch(`missing declaration for ${symbol.getName()}`)
  }
  return checker.getTypeOfSymbolAtLocation(symbol, declaration)
}

export function literalValue(checker: ts.TypeChecker, type: ts.Type): number | string | undefined {
  if (type.isLiteral()) {
    return typeof type.value === "object" ? undefined : type.value
  }
  const text = checker.typeToString(type)
  return /^".*"$/.test(text) ? text.slice(1, -1) : undefined
}

export function assertEnum(
  checker: ts.TypeChecker,
  actual: ts.Type,
  expected: readonly string[],
  path: string,
): void {
  assertEnumMembers(checker, actual.isUnion() ? actual.types : [actual], expected, path)
}

function assertEnumMembers(
  checker: ts.TypeChecker,
  members: readonly ts.Type[],
  expected: readonly string[],
  path: string,
): void {
  const values = members.map((member) => literalValue(checker, member)).sort()
  const expectedValues = [...expected].sort()
  if (
    values.length !== expectedValues.length ||
    values.some((value, index) => value !== expectedValues[index])
  ) {
    throw new SchemaTypeMismatch(`${path}: enum mismatch`)
  }
}

/**
 * Returns the non-null member of a nullable type for recursive checking, or `undefined`
 * once a nullable enum, which flattens into one union of its literals and null, is checked.
 */
export function nullableValue(
  checker: ts.TypeChecker,
  actual: ts.Type,
  expected: SchemaNode,
  path: string,
): ts.Type | undefined {
  const members = actual.isUnion() ? actual.types : [actual]
  const valueMembers = members.filter((member) => (member.flags & ts.TypeFlags.Null) === 0)
  if (expected.type === "enum" && members.length === valueMembers.length + 1) {
    assertEnumMembers(checker, valueMembers, expected.values ?? [], path)
    return undefined
  }
  const value = valueMembers[0]
  if (members.length !== 2 || valueMembers.length !== 1 || value === undefined) {
    throw new SchemaTypeMismatch(`${path}: expected nullable type`)
  }
  return value
}

export function assertFlag(actual: ts.Type, flag: ts.TypeFlags, path: string, name: string): void {
  if ((actual.flags & flag) === 0) {
    throw new SchemaTypeMismatch(`${path}: expected ${name}`)
  }
}

export function containsFlag(actual: ts.Type, flag: ts.TypeFlags): boolean {
  return (
    (actual.flags & flag) !== 0 ||
    (actual.isIntersection() && actual.types.some((type) => containsFlag(type, flag)))
  )
}

export function requiredNode(
  values: Readonly<Record<string, SchemaNode>> | undefined,
  name: string,
): SchemaNode {
  const value = values?.[name]
  if (value === undefined) {
    throw new SchemaTypeMismatch(`schema is missing ${name}`)
  }
  return value
}
