module github.com/nib-editor/nib/plugins/test/go

go 1.24

require (
	github.com/nib-editor/nib/sdk/go v0.0.0
	go.bytecodealliance.org/cm v0.3.0
)

replace github.com/nib-editor/nib/sdk/go => ../../../sdk/go
