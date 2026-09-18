import { readFile } from "node:fs/promises"
import { resolve } from "node:path"

function channel(value: number): number {
  const normalized = value / 255
  return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4
}

function luminance(hex: string): number {
  const channels = hex.match(/[0-9a-f]{2}/gi)?.map((part) => channel(Number.parseInt(part, 16)))
  const [red, green, blue] = channels ?? []
  if (red === undefined || green === undefined || blue === undefined) {
    throw new TypeError(`Invalid colour token: ${hex}`)
  }
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue
}

function contrast(foreground: string, background: string): number {
  const [lighter, darker] = [luminance(foreground), luminance(background)].sort(
    (left, right) => right - left,
  )
  if (lighter === undefined || darker === undefined) throw new TypeError("Missing luminance")
  return (lighter + 0.05) / (darker + 0.05)
}

/** Returns the declaration block that follows `marker`, without nested rules. */
function block(source: string, marker: string): string {
  const start = source.indexOf(marker)
  if (start < 0) throw new TypeError(`Missing block: ${marker}`)
  const open = start + marker.length
  const end = source.indexOf("}", open)
  if (end < 0) throw new TypeError(`Unterminated block: ${marker}`)
  return source.slice(open, end)
}

function palette(source: string): Map<string, string> {
  const entries = source.matchAll(/(--[a-z0-9-]+):\s*(#[0-9a-f]{6})/gi)
  return new Map(Array.from(entries, (match) => [match[1] ?? "", match[2] ?? ""]))
}

function token(modePalette: Map<string, string>, name: string): string {
  const value = modePalette.get(name)
  if (value === undefined) throw new TypeError(`Missing ${name}`)
  return value
}

const SURFACES = ["--bg", "--panel", "--panel-2"]
const READABLE = [
  "--text",
  "--text-muted",
  "--text-dim",
  "--accent",
  "--observed-direct",
  "--observed-relay",
  "--observed-failed",
  "--observed-none",
]
const SOFT_PAIRS = [
  ["--accent", "--accent-soft"],
  ["--observed-direct", "--observed-direct-soft"],
  ["--observed-relay", "--observed-relay-soft"],
  ["--observed-failed", "--observed-failed-soft"],
  ["--observed-none", "--observed-none-soft"],
]

async function stylesheet(): Promise<string> {
  return readFile(resolve(process.cwd(), "src/styles/tokens.css"), "utf8")
}

async function modes(): Promise<ReadonlyArray<readonly [string, Map<string, string>]>> {
  const source = await stylesheet()
  return [
    ["light", palette(block(source, ":root {"))],
    ["dark", palette(block(source, ':root[data-theme="dark"] {'))],
  ]
}

test("the two dark blocks declare identical values", async () => {
  const source = await stylesheet()
  const automatic = palette(block(source, ':root:not([data-theme="light"]) {'))
  const explicit = palette(block(source, ':root[data-theme="dark"] {'))

  expect(Object.fromEntries(automatic)).toEqual(Object.fromEntries(explicit))
  expect(automatic.size).toBeGreaterThan(15)
})

test("every readable token reaches AA on every surface, in both modes", async () => {
  for (const [mode, values] of await modes()) {
    for (const name of READABLE) {
      for (const surface of SURFACES) {
        const ratio = contrast(token(values, name), token(values, surface))
        expect(`${mode} ${name} on ${surface}: ${ratio.toFixed(2)}`).toBe(
          `${mode} ${name} on ${surface}: ${Math.max(ratio, 4.5).toFixed(2)}`,
        )
      }
    }
  }
})

test("soft fills keep their own foreground at AA, in both modes", async () => {
  for (const [mode, values] of await modes()) {
    for (const [foreground, background] of SOFT_PAIRS) {
      const ratio = contrast(token(values, foreground ?? ""), token(values, background ?? ""))
      expect(`${mode} ${foreground}: ${ratio.toFixed(2)}`).toBe(
        `${mode} ${foreground}: ${Math.max(ratio, 4.5).toFixed(2)}`,
      )
    }
  }
})

test("accent text stays legible on the accent fill, in both modes", async () => {
  for (const [, values] of await modes()) {
    expect(contrast(token(values, "--accent-contrast"), token(values, "--accent"))).toBeGreaterThan(
      4.5,
    )
  }
})

test("control boundaries reach the 3:1 non-text minimum on every surface", async () => {
  for (const [mode, values] of await modes()) {
    for (const surface of SURFACES) {
      const ratio = contrast(token(values, "--control"), token(values, surface))
      expect(`${mode} --control on ${surface}: ${ratio.toFixed(2)}`).toBe(
        `${mode} --control on ${surface}: ${Math.max(ratio, 3).toFixed(2)}`,
      )
    }
  }
})

test("each mode declares its own color-scheme so native controls follow", async () => {
  const source = await stylesheet()

  expect(block(source, ":root {")).toMatch(/color-scheme:\s*light/)
  expect(block(source, ':root[data-theme="dark"] {')).toMatch(/color-scheme:\s*dark/)
  expect(block(source, ':root:not([data-theme="light"]) {')).toMatch(/color-scheme:\s*dark/)
})
