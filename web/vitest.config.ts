import { defineConfig } from "vitest/config"

const config = defineConfig({
  test: {
    coverage: {
      provider: "v8",
      reporter: ["text", "json-summary"],
    },
    environment: "jsdom",
    globals: true,
    passWithNoTests: true,
  },
})

export default config
