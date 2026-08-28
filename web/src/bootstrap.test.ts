import { MissingApplicationRootError, requireApplicationRoot } from "./bootstrap"

test("throws a typed error when the application root is missing", () => {
  expect(() => requireApplicationRoot(null)).toThrow(MissingApplicationRootError)
})
