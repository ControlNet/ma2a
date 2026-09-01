# Todo 12 Full Endpoint Data

- Pinned Iroh 1.1 `EndpointData` preserves ordered deduplicated `TransportAddr` values and optional
  `UserData`; MA2A signed data must remain identity-free and use `EndpointInfo::from_parts` at lookup.
- `EndpointAddr` itself is a `BTreeSet`, so snapshot publication can preserve only the order exposed
  by that boundary. Direct conversion from Iroh `EndpointData` preserves its original priority order.
- Cross-Space lookup order is the caller-supplied authorization snapshot order. Dedupe retains the
  first address; disagreement between `None`, `Some("")`, or different user data fails closed.
- Low-cardinality observability is implemented as closed enums and atomic counter snapshots without
  arbitrary strings or topology-bearing labels.
