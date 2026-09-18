import {
  applyTheme,
  isThemeMode,
  prefersDarkNow,
  readStoredTheme,
  resolveTheme,
  storeTheme,
  THEME_STORAGE_KEY,
} from "./theme"

function memoryStorage(initial?: string): Storage {
  const values = new Map<string, string>()
  if (initial !== undefined) values.set(THEME_STORAGE_KEY, initial)
  return {
    get length() {
      return values.size
    },
    clear: () => values.clear(),
    getItem: (key: string) => values.get(key) ?? null,
    key: (index: number) => Array.from(values.keys())[index] ?? null,
    removeItem: (key: string) => void values.delete(key),
    setItem: (key: string, value: string) => void values.set(key, value),
  }
}

function blockedStorage(): Storage {
  const reject = (): never => {
    throw new DOMException("blocked", "SecurityError")
  }
  return {
    get length(): number {
      return reject()
    },
    clear: reject,
    getItem: reject,
    key: reject,
    removeItem: reject,
    setItem: reject,
  }
}

test("accepts only the three known modes", () => {
  expect(isThemeMode("system")).toBe(true)
  expect(isThemeMode("dark")).toBe(true)
  expect(isThemeMode("sepia")).toBe(false)
  expect(isThemeMode(null)).toBe(false)
})

test("falls back to system when storage is empty, corrupt, blocked or absent", () => {
  expect(readStoredTheme(memoryStorage())).toBe("system")
  expect(readStoredTheme(memoryStorage("sepia"))).toBe("system")
  expect(readStoredTheme(blockedStorage())).toBe("system")
  expect(readStoredTheme(undefined)).toBe("system")
  expect(readStoredTheme(memoryStorage("dark"))).toBe("dark")
})

test("storing system clears the key rather than pinning a value", () => {
  const storage = memoryStorage("dark")

  storeTheme("system", storage)

  expect(storage.getItem(THEME_STORAGE_KEY)).toBeNull()
})

test("storing an explicit mode never throws on blocked storage", () => {
  expect(() => storeTheme("dark", blockedStorage())).not.toThrow()
  expect(() => storeTheme("system", blockedStorage())).not.toThrow()
})

test("system resolves through the media preference, explicit modes do not", () => {
  expect(resolveTheme("system", true)).toBe("dark")
  expect(resolveTheme("system", false)).toBe("light")
  expect(resolveTheme("light", true)).toBe("light")
  expect(resolveTheme("dark", false)).toBe("dark")
})

test("system removes the attribute so the stylesheet media query decides", () => {
  const root = document.createElement("html")

  applyTheme("dark", root)
  expect(root.getAttribute("data-theme")).toBe("dark")

  applyTheme("light", root)
  expect(root.getAttribute("data-theme")).toBe("light")

  applyTheme("system", root)
  expect(root.hasAttribute("data-theme")).toBe(false)
})

test("a missing root or matchMedia never throws", () => {
  expect(() => applyTheme("dark", undefined)).not.toThrow()
  expect(prefersDarkNow(undefined)).toBe(false)
})
