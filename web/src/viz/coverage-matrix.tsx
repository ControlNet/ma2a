import type { ReactNode } from "react"

export type CoverageRow = {
  readonly id: string
  readonly name: string
  readonly detail: string
  readonly covers: readonly boolean[]
  readonly compatible: boolean
}

export type CoverageSpace = string | { readonly id: string; readonly name: string }

function spaceKey(space: CoverageSpace, index: number): string {
  return typeof space === "string" ? `${index}:${space}` : space.id
}

function spaceLabel(space: CoverageSpace): string {
  return typeof space === "string" ? space : space.name
}

/**
 * `home_relay_compatible` is exactly "this candidate covers every active Space",
 * so the verdict and its reason are drawn as one grid rather than two claims.
 */
export function CoverageMatrix({
  spaces,
  rows,
  caption,
}: {
  readonly spaces: readonly CoverageSpace[]
  readonly rows: readonly CoverageRow[]
  readonly caption: string
}): ReactNode {
  return (
    <table className="matrix">
      <caption className="visually-hidden">{caption}</caption>
      <thead>
        <tr>
          <th scope="col">Candidate</th>
          {spaces.map((space, index) => (
            <th key={spaceKey(space, index)} scope="col">
              {spaceLabel(space)}
            </th>
          ))}
          <th scope="col">Home-compatible</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.id}>
            <th scope="row">
              <span className="matrix__name">{row.name}</span>
              <span className="matrix__detail">{row.detail}</span>
            </th>
            {row.covers.map((covered, index) => {
              const space = spaces[index]
              const cellKey =
                space === undefined ? `${row.id}-${index}` : `${row.id}-${spaceKey(space, index)}`
              return (
                <td className="matrix__cell" key={cellKey}>
                  <span className={covered ? "trust__mark" : "trust__mark trust__mark--absent"}>
                    <span className="visually-hidden">{covered ? "covered" : "not covered"}</span>
                  </span>
                </td>
              )
            })}
            <td className="matrix__cell">
              <span
                className={
                  row.compatible ? "matrix__verdict" : "matrix__verdict matrix__verdict--no"
                }
              >
                {row.compatible ? "YES" : "NO"}
              </span>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
