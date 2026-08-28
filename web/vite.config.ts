import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

const config = defineConfig({
  build: {
    emptyOutDir: true,
    outDir: "dist",
    sourcemap: false,
  },
  plugins: [react(), tailwindcss()],
})

export default config
