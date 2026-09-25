; Text objects, for keys such as `maf` and `]f` in the Helix keymap. The
; capture names follow Helix's, so keymaps work the same in every language.

(function_definition
  body: (_) @function.inside) @function.around

(lambda
  body: (_) @function.inside) @function.around

(class_definition
  body: (_) @class.inside) @class.around

; A parameter with the comma after it, if any.
(parameters (_) @parameter.inside @parameter.around . ","? @parameter.around)
(lambda_parameters (_) @parameter.inside @parameter.around . ","? @parameter.around)
(argument_list (_) @parameter.inside @parameter.around . ","? @parameter.around)

(comment) @comment.inside
; Consecutive comments make one comment.
(comment)+ @comment.around

; pytest's convention.
((function_definition
  name: (identifier) @_name
  body: (_) @test.inside) @test.around
  (#match? @_name "^test"))
