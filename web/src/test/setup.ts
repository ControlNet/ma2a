import { JSDOM } from "jsdom"

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost/",
})

Object.defineProperties(globalThis, {
  CustomEvent: { configurable: true, value: dom.window.CustomEvent },
  DocumentFragment: { configurable: true, value: dom.window.DocumentFragment },
  Element: { configurable: true, value: dom.window.Element },
  Event: { configurable: true, value: dom.window.Event },
  HTMLElement: { configurable: true, value: dom.window.HTMLElement },
  KeyboardEvent: { configurable: true, value: dom.window.KeyboardEvent },
  MouseEvent: { configurable: true, value: dom.window.MouseEvent },
  MutationObserver: { configurable: true, value: dom.window.MutationObserver },
  Node: { configurable: true, value: dom.window.Node },
  document: { configurable: true, value: dom.window.document },
  getComputedStyle: {
    configurable: true,
    value: dom.window.getComputedStyle.bind(dom.window),
  },
  navigator: { configurable: true, value: dom.window.navigator },
  window: { configurable: true, value: dom.window },
})
