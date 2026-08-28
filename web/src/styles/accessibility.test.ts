import { readFile } from "node:fs/promises"
import { resolve } from "node:path"

test("loads reduced-motion overrides after every visual stylesheet", async () => {
  const stylesheet = await readFile(resolve(process.cwd(), "src/styles.css"), "utf8")
  const imports = Array.from(stylesheet.matchAll(/@import\s+"([^"]+)"/g), (match) => match[1])

  expect(imports.at(-1)).toBe("./styles/accessibility.css")
})

test("uses an AA text token for the sidebar runtime status", async () => {
  const layout = await readFile(resolve(process.cwd(), "src/styles/layout.css"), "utf8")

  expect(layout).toMatch(/\.sidebar-foot span\s*{[^}]*color:\s*var\(--text-secondary\)/s)
})
