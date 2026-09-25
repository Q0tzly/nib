; Text objects, for keys such as `maf` and `]f` in the Helix keymap. The
; capture names follow Helix's, so keymaps work the same in every language.

(function_item
  body: (_) @function.inside) @function.around

(function_signature_item) @function.around

(closure_expression
  body: (_) @function.inside) @function.around

[
  (struct_item)
  (enum_item)
  (union_item)
  (trait_item)
  (impl_item)
  (type_item)
] @class.around

(struct_item body: (_) @class.inside)
(enum_item body: (_) @class.inside)
(union_item body: (_) @class.inside)
(trait_item body: (_) @class.inside)
(impl_item body: (_) @class.inside)

; A parameter with the comma after it, if any.
(parameters (_) @parameter.inside @parameter.around . ","? @parameter.around)
(closure_parameters (_) @parameter.inside @parameter.around . ","? @parameter.around)
(arguments (_) @parameter.inside @parameter.around . ","? @parameter.around)
(type_parameters (_) @parameter.inside @parameter.around . ","? @parameter.around)
(type_arguments (_) @parameter.inside @parameter.around . ","? @parameter.around)

(line_comment) @comment.inside
(block_comment) @comment.inside
; Consecutive line comments make one comment.
(line_comment)+ @comment.around
(block_comment) @comment.around

; A #[test] function, with its attribute.
(
  (attribute_item (attribute (identifier) @_test)) @test.around
  .
  (function_item body: (_) @test.inside) @test.around
  (#eq? @_test "test")
)
