import { readdir, readFile, stat } from "node:fs/promises"
import { resolve } from "node:path"

async function stylesheets(): Promise<readonly string[]> {
  const directory = resolve(process.cwd(), "src/styles")
  const names = (await readdir(directory)).filter((name) => name.endsWith(".css"))
  const sources = await Promise.all(names.map((name) => readFile(resolve(directory, name), "utf8")))
  return [...sources, await readFile(resolve(process.cwd(), "src/styles.css"), "utf8")]
}

test("no stylesheet reaches outside the loopback origin", async () => {
  for (const source of await stylesheets()) {
    expect(source).not.toMatch(/url\(\s*["']?https?:/i)
    expect(source).not.toMatch(/@import\s+(url\()?["']https?:/i)
  }
})

test("both families are declared as self-hosted woff2 faces", async () => {
  const source = await readFile(resolve(process.cwd(), "src/styles/fonts.css"), "utf8")
  const families = Array.from(
    source.matchAll(/font-family:\s*"([^"]+)"/g),
    (match) => match[1] ?? "",
  )

  expect(families).toEqual(["Space Grotesk Variable", "JetBrains Mono Variable"])
  expect(source.match(/\.woff2"\)/g)).toHaveLength(2)
})

test("only the latin subset ships, so the embedded binary stays small", async () => {
  const source = await readFile(resolve(process.cwd(), "src/styles/fonts.css"), "utf8")
  const files = Array.from(source.matchAll(/url\("([^"]+)"\)/g), (match) => match[1] ?? "")

  expect(files).toHaveLength(2)
  for (const file of files) {
    expect(file).toContain("-latin-")
    const bytes = await stat(resolve(process.cwd(), "node_modules", file))
    expect(bytes.size).toBeGreaterThan(0)
  }
})

test("the token layer points both font stacks at the self-hosted families", async () => {
  const tokens = await readFile(resolve(process.cwd(), "src/styles/tokens.css"), "utf8")

  expect(tokens).toMatch(/--font-sans:\s*"Space Grotesk Variable",/)
  expect(tokens).toMatch(/--font-mono:\s*"JetBrains Mono Variable",/)
})
