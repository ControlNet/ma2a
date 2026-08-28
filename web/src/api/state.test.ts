import {
  beginResnapshot,
  installSnapshot,
  markDisconnected,
  type RuntimeState,
  receiveRevision,
} from "./state"

describe("runtime revision state", () => {
  test("accepts exactly the next revision when the snapshot is authoritative", () => {
    const given = installSnapshot(10)

    const when = receiveRevision(given, 11)

    expect(when).toEqual({
      accepted: true,
      effect: "none",
      state: { kind: "ready", revision: 11 },
    })
  })

  test("marks a revision gap uncertain and requests one resnapshot", () => {
    const given = installSnapshot(10)

    const when = receiveRevision(given, 12)

    expect(when).toEqual({
      accepted: false,
      effect: "resnapshot",
      state: {
        kind: "uncertain",
        lastRevision: 10,
        reason: "revision_gap",
        resnapshot: "required",
      },
    })
  })

  test("discards later events while recovery is required without requesting another snapshot", () => {
    const given = receiveRevision(installSnapshot(10), 12).state

    const when = receiveRevision(given, 13)

    expect(when).toEqual({ accepted: false, effect: "none", state: given })
  })

  test("keeps recovery bounded after the snapshot request starts", () => {
    const uncertain = receiveRevision(installSnapshot(10), 12).state
    const given = beginResnapshot(uncertain)

    const when = beginResnapshot(given)

    expect(when).toEqual(given)
    expect(receiveRevision(when, 13)).toEqual({
      accepted: false,
      effect: "none",
      state: given,
    })
  })

  test.each([
    [10, "duplicate_revision"],
    [9, "out_of_order_revision"],
  ] as const)("marks revision %i uncertain as %s", (revision, reason) => {
    const given = installSnapshot(10)

    const when = receiveRevision(given, revision)

    expect(when).toEqual({
      accepted: false,
      effect: "resnapshot",
      state: { kind: "uncertain", lastRevision: 10, reason, resnapshot: "required" },
    })
  })

  test("marks a disconnect uncertain and requests one resnapshot", () => {
    const given = installSnapshot(10)

    const when = markDisconnected(given)

    expect(when).toEqual({
      accepted: false,
      effect: "resnapshot",
      state: {
        kind: "uncertain",
        lastRevision: 10,
        reason: "disconnected",
        resnapshot: "required",
      },
    })
  })

  test("a replacement snapshot restores authority from its own revision", () => {
    const uncertain: RuntimeState = receiveRevision(installSnapshot(10), 12).state

    const when = installSnapshot(20)

    expect(uncertain.kind).toBe("uncertain")
    expect(receiveRevision(when, 21).accepted).toBe(true)
  })
})
