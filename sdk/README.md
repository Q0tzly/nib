# sdk

Plugin SDKs, one directory per language (`rust/`, `go/`, ...). SDKs are versioned independently of the editor: an SDK version changes only when the API in `api/` changes. The one exception is a change that makes the SDK unreachable at its current version, such as the Go module path changing; that gets a patch release.

The Go SDK lives in its own module under `sdk/go/`, so its tags are prefixed with the directory, e.g. `sdk/go/v0.4.1`.
