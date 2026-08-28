export class MissingApplicationRootError extends Error {
  constructor(rootId: string) {
    super(`Missing application root element: ${rootId}`)
    this.name = "MissingApplicationRootError"
  }
}

export function requireApplicationRoot(root: HTMLElement | null): HTMLElement {
  if (root === null) {
    throw new MissingApplicationRootError("root")
  }
  return root
}
