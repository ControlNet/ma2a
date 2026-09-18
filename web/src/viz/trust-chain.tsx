import type { ReactNode } from "react"

export type Boundary = {
  readonly name: string
  readonly guard: string
  readonly holds: readonly boolean[]
}

/** Which secret is allowed to exist at which boundary. Rows are secrets. */
export function TrustChain({
  secrets,
  boundaries,
}: {
  readonly secrets: readonly string[]
  readonly boundaries: readonly Boundary[]
}): ReactNode {
  return (
    <table className="trust">
      <caption className="visually-hidden">Where each secret is allowed to exist</caption>
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
              const allowed = boundary.holds[row] === true
              return (
                <td className="trust__cell" key={`${boundary.name}-${secret}`}>
                  <span className={allowed ? "trust__mark" : "trust__mark trust__mark--absent"}>
                    <span className="visually-hidden">
                      {allowed ? "may exist here" : "never exists here"}
                    </span>
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
