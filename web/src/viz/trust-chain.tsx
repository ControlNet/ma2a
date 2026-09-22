import type { ReactNode } from "react"

/** How a secret relates to one boundary. Handling is not retention. */
export type SecretPresence = "never" | "transient" | "retained"

export type Boundary = {
  readonly name: string
  readonly guard: string
  readonly presence: readonly SecretPresence[]
}

const MARK: Record<SecretPresence, { readonly className: string; readonly reading: string }> = {
  never: { className: "trust__mark trust__mark--never", reading: "never present here" },
  transient: {
    className: "trust__mark trust__mark--transient",
    reading: "handled transiently, not retained",
  },
  retained: { className: "trust__mark trust__mark--retained", reading: "retained here" },
}

/**
 * Where each secret is *retained*, and where it is merely handled on the way
 * through. Signing in necessarily passes a password through the browser; the
 * point is that nothing keeps it there.
 */
export function TrustChain({
  secrets,
  boundaries,
}: {
  readonly secrets: readonly string[]
  readonly boundaries: readonly Boundary[]
}): ReactNode {
  return (
    <table className="trust">
      <caption className="visually-hidden">
        Where each secret is retained, and where it is only handled in passing
      </caption>
      <thead>
        <tr>
          <th scope="col">Secret</th>
          {boundaries.map((boundary) => (
            <th key={boundary.name} scope="col">
              <span className="trust__name">{boundary.name}</span>
              <span className="trust__guard">{boundary.guard}</span>
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {secrets.map((secret, row) => (
          <tr key={secret}>
            <th scope="row">{secret}</th>
            {boundaries.map((boundary) => {
              const mark = MARK[boundary.presence[row] ?? "never"]
              return (
                <td className="trust__cell" key={`${boundary.name}-${secret}`}>
                  <span className={mark.className}>
                    <span className="visually-hidden">{mark.reading}</span>
                  </span>
                </td>
              )
            })}
          </tr>
        ))}
      </tbody>
    </table>
  )
}
