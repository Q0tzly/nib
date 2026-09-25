; Text objects, for keys such as `maf` and `]f` in the Helix keymap. The
; capture names follow Helix's, so keymaps work the same in every language.

(function_definition
  body: (_) @function.inside) @function.around

(comment) @comment.inside
; Consecutive comments make one comment.
(comment)+ @comment.around
