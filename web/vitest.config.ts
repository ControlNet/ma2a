import { defineConfig } from "vitest/config"

const config = defineConfig({
  test: {
    environment: "node",
    globals: true,
    passWithNoTests: true,
  },
})

export default config
