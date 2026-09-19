import { readdir, readFile } from "node:fs/promises"
import { join, relative, resolve } from "node:path"

const ROOT = resolve(process.cwd(), "src")

/** Only the theme preference may touch browser storage, and it is not a secret. */
const STORAGE_ALLOWLIST = ["theme/use-theme.ts"]

/** The bearer is HttpOnly; only the deliberately script-readable CSRF cookie is read. */
const COOKIE_ALLOWLIST = ["api/web-auth.ts"]

async function sources(): Promise<ReadonlyArray<readonly [string, string]>> {
  const found: [string, string][] = []
  const walk = async (directory: string): Promise<void> => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) {
        await walk(path)
        continue
      }
      if (!/\.tsx?$/.test(entry.name) || entry.name.includes(".test.")) continue
      found.push([relative(ROOT, path), await readFile(path, "utf8")])
    }
  }
  await walk(ROOT)
  return found
}

test("only the theme preference reaches browser storage", async () => {
  const offenders = (await sources())
    .filter(([, source]) => /\.(localStorage|sessionStorage|indexedDB)\b/.test(source))
    .map(([path]) => path)

  expect(offenders.toSorted()).toEqual(STORAGE_ALLOWLIST.toSorted())
})

test("nothing secret is written to storage or put in a URL", async () => {
  for (const [path, source] of await sources()) {
    const stored = /setItem\([^)]*(password|passphrase|bearer|invite|secret)/i.test(source)
    const urled =
      /(location\.(search|hash)\s*=|searchParams\.set\()[^\n]*(password|passphrase|bearer|invite|secret)/i.test(
        source,
      )
    expect(`${path} stores a secret: ${stored}`).toBe(`${path} stores a secret: false`)
    expect(`${path} puts a secret in a URL: ${urled}`).toBe(`${path} puts a secret in a URL: false`)
  }
})

test("the only cookie a script reads is the CSRF value, never the session bearer", async () => {
  const readers = (await sources())
    .filter(([, source]) => /document\.cookie/.test(source))
    .map(([path]) => path)

  expect(readers.toSorted()).toEqual(COOKIE_ALLOWLIST.toSorted())
  const [, webAuth] = (await sources()).find(([path]) => path === "api/web-auth.ts") ?? []
  expect(webAuth).toMatch(/name === CSRF_COOKIE/)
  expect(webAuth).not.toMatch(/ma2a_session/)
})
