# sdk

Plugin SDKs, one directory per language (`rust/`, `go/`, ...). SDKs are versioned independently of the editor: an SDK version changes only when the API in `api/` changes.

A Go SDK lives in its own module under `sdk/go/`, so its tags must be prefixed with the directory, e.g. `sdk/go/v0.1.0`.
