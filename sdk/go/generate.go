package nib

// The bindings in nib/plugin/ are generated from wit/deps/nib-plugin/, a
// copy of the repository's api/wit/. After changing the API, copy it there
// and run `go generate`. The rest of wit/ adds the WASI imports TinyGo's
// runtime needs to the world plugins are built for.
//go:generate go run go.bytecodealliance.org/cmd/wit-bindgen-go@v0.7.0 generate --world plugin --out . ./wit/deps/nib-plugin
