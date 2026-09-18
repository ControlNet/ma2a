import { readdir, readFile } from "node:fs/promises"
import { resolve } from "node:path"

test("loads reduced-motion overrides after every visual stylesheet", async () => {
  const stylesheet = await readFile(resolve(process.cwd(), "src/styles.css"), "utf8")
  const imports = Array.from(stylesheet.matchAll(/@import\s+"([^"]+)"/g), (match) => match[1])

  expect(imports.at(-1)).toBe("./styles/accessibility.css")
})

test("no stylesheet still depends on a pre-redesign token name", async () => {
  const directory = resolve(process.cwd(), "src/styles")
  const names = (await readdir(directory)).filter((name) => name.endsWith(".css"))
  for (const name of names) {
    const source = await readFile(resolve(directory, name), "utf8")
    expect(source).not.toMatch(/var\(--(surface|status|border|accent-primary|focus-ring)/)
  }
})

test("select controls inherit the shared application font", async () => {
  const stylesheet = await readFile(resolve(process.cwd(), "src/styles/base.css"), "utf8")

  expect(stylesheet).toMatch(/button,\s*input,\s*select,\s*textarea\s*{[^}]*font:\s*inherit/s)
})
