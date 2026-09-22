# Quickstart

MA2A is distributed as one archive containing one self-contained `ma2a` executable and its
operator documentation. Extract the archive into a new directory and run the binary from there.

## Verify the download

Download the archive, its `.sha256` file, matching `.spdx.json` SBOM, dependency/license report,
and Sigstore attestation bundles from the GitHub release. From the directory containing those files:

```sh
sha256sum --check ma2a-app-x86_64-unknown-linux-gnu.tar.xz.sha256
gh attestation verify ma2a-app-x86_64-unknown-linux-gnu.tar.xz \
  --repo ControlNet/ma2a \
  --signer-workflow ControlNet/ma2a/.github/workflows/release.yml
```

On macOS use `shasum -a 256 -c ARCHIVE.sha256`. On Windows use
`Get-FileHash -Algorithm SHA256 ARCHIVE` and compare the result with `ARCHIVE.sha256`.

## Start an isolated Runtime

The examples use an explicit state directory so they do not affect another MA2A Runtime.

```sh
mkdir -p "$HOME/.local/state/ma2a-quickstart"
chmod 700 "$HOME/.local/state/ma2a-quickstart"
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" --version
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" start
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" status
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" ui init
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" ui start
```

`start` launches the daemon in the background with WebUI stopped. `status` queries the running
daemon and errors if it is stopped. `ui init` reads and
confirms the Web password without echo. Passwords must contain 1–1,024 UTF-8 bytes, and both
entries must match exactly. No character-class combination is required.
`ui start` starts WebUI in the background and prints its URL. Open that URL in a browser.
Use `ui status` to query it or `ui stop` to stop only WebUI. Stop the isolated Runtime with:

```sh
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" stop
```

## Create a Space

```sh
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" start
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" space create primary
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" space list
```

The name you give is shared Space metadata: every Endpoint that joins reads `primary` without
configuring anything. Commands that name a Space accept either that name or the full Space ID.

## Invite a second Endpoint

An invitation is single-use, owner-approved, and short-lived (five minutes by default). `space
invite` prints only the ticket, and `space accept` reads one without ever taking it from the
command line:

```sh
./ma2a --state-dir "$HOME/.local/state/ma2a-peer" start
./ma2a --state-dir "$HOME/.local/state/ma2a-quickstart" space invite primary |
  ./ma2a --state-dir "$HOME/.local/state/ma2a-peer" space accept
```

Run `space accept` on its own to be prompted for a ticket instead; the prompt does not echo it.

A member can hand its membership back, which asks the Space authority to sign a new generation
removing it:

```sh
./ma2a --state-dir "$HOME/.local/state/ma2a-peer" space leave primary
```

See [operations.md](operations.md) for lifecycle, backup boundaries, relay configuration, and
troubleshooting. See [security.md](security.md) before exposing a Private Relay.
