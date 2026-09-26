; Written for nib. The code of a fenced block, in the language its info
; string names ("rust", or a file type such as "rs").
(fenced_code_block
  (info_string
    (language) @injection.language)
  (code_fence_content) @injection.content)

; Front matter.
((minus_metadata) @injection.content
 (#set! injection.language "yaml"))

((plus_metadata) @injection.content
 (#set! injection.language "toml"))
