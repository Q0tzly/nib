; Text objects, for keys such as `maf` and `]f` in the Helix keymap. The
; capture names follow Helix's, so keymaps work the same in every language.

; Members and elements, with the comma after them, if any.
(object (pair) @parameter.inside @parameter.around . ","? @parameter.around)
(array (_) @parameter.inside @parameter.around . ","? @parameter.around)

(comment) @comment.inside
; Consecutive comments make one comment.
(comment)+ @comment.around
