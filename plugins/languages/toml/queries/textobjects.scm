; Text objects, for keys such as `maf` and `]f` in the Helix keymap. The
; capture names follow Helix's, so keymaps work the same in every language.

; Tables, so `]t` goes from one to the next.
(table) @class.around
(table_array_element) @class.around

(comment) @comment.inside
; Consecutive comments make one comment.
(comment)+ @comment.around
