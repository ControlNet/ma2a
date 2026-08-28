import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import { requireApplicationRoot } from "./bootstrap"
import "./styles.css"

if (import.meta.env.DEV && import.meta.env["VITE_DISABLE_REACT_DEVTOOLS"] !== "1") {
  void import("react-grab")
  void import("react-scan")
}

createRoot(requireApplicationRoot(document.getElementById("root"))).render(<StrictMode />)
