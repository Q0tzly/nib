; Written for nib. Doc comments are Markdown. All of a file's doc comments
; make one document, so a code block can run over several lines.
((line_comment
  (doc_comment) @injection.content)
 (#set! injection.language "markdown")
 (#set! injection.combined))

((block_comment
  (doc_comment) @injection.content)
 (#set! injection.language "markdown")
 (#set! injection.combined))
