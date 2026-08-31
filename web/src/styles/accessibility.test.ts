import { readFile } from "node:fs/promises"
import { resolve } from "node:path"

function channel(value: number): number {
  const normalized = value / 255
  return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4
}

function luminance(hex: string): number {
  const channels = hex.match(/[0-9a-f]{2}/gi)?.map((value) => channel(Number.parseInt(value, 16)))
  if (channels?.length !== 3) throw new TypeError(`Invalid color token: ${hex}`)
  const [red, green, blue] = channels
  if (red === undefined || green === undefined || blue === undefined) {
    throw new TypeError(`Invalid color token: ${hex}`)
  }
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue
}

function contrast(foreground: string, background: string): number {
  const values = [luminance(foreground), luminance(background)].sort((left, right) => right - left)
  const lighter = values[0]
  const darker = values[1]
  if (lighter === undefined || darker === undefined) throw new TypeError("Missing contrast value")
  return (lighter + 0.05) / (darker + 0.05)
}

function token(stylesheet: string, name: string): string {
  const value = new RegExp(`${name}:\\s*(#[0-9a-f]{6})`, "i").exec(stylesheet)?.[1]
  if (value === undefined) throw new TypeError(`Missing ${name}`)
  return value
}

test("loads reduced-motion overrides after every visual stylesheet", async () => {
  const stylesheet = await readFile(resolve(process.cwd(), "src/styles.css"), "utf8")
  const imports = Array.from(stylesheet.matchAll(/@import\s+"([^"]+)"/g), (match) => match[1])

  expect(imports.at(-1)).toBe("./styles/accessibility.css")
})

test("uses an AA text token for the sidebar runtime status", async () => {
  const layout = await readFile(resolve(process.cwd(), "src/styles/layout.css"), "utf8")

  expect(layout).toMatch(/\.sidebar-foot span\s*{[^}]*color:\s*var\(--text-secondary\)/s)
})

test("keeps small muted metadata at AA contrast on both content surfaces", async () => {
  const stylesheet = await readFile(resolve(process.cwd(), "src/styles/tokens.css"), "utf8")
  const muted = token(stylesheet, "--text-muted")

  expect(contrast(muted, token(stylesheet, "--surface-primary"))).toBeGreaterThanOrEqual(4.5)
  expect(contrast(muted, token(stylesheet, "--surface-secondary"))).toBeGreaterThanOrEqual(4.5)
})

test("select controls inherit the shared application font", async () => {
  const stylesheet = await readFile(resolve(process.cwd(), "src/styles/tokens.css"), "utf8")

  expect(stylesheet).toMatch(/button,\s*input,\s*select,\s*textarea\s*{[^}]*font:\s*inherit/s)
})
