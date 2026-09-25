; Text objects, for keys such as `maf` and `]f` in the Helix keymap. The
; capture names follow Helix's, so keymaps work the same in every language.

(function_declaration) @function.around
(function_declaration body: (_) @function.inside)
(method_declaration) @function.around
(method_declaration body: (_) @function.inside)
(func_literal) @function.around
(func_literal body: (_) @function.inside)

(type_declaration) @class.around
(type_spec type: (struct_type (field_declaration_list) @class.inside))
(type_spec type: (interface_type) @class.inside)

; A parameter with the comma after it, if any.
(parameter_list (_) @parameter.inside @parameter.around . ","? @parameter.around)
(argument_list (_) @parameter.inside @parameter.around . ","? @parameter.around)

(comment) @comment.inside
; Consecutive comments make one comment.
(comment)+ @comment.around

; `go test`'s convention.
((function_declaration
  name: (identifier) @_name) @test.around
  (#match? @_name "^(Test|Benchmark|Fuzz|Example)"))
((function_declaration
  name: (identifier) @_name
  body: (_) @test.inside)
  (#match? @_name "^(Test|Benchmark|Fuzz|Example)"))
