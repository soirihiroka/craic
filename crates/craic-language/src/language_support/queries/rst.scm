(comment) @comment

(title) @text.title
"adornment" @punctuation.special

[(target) (reference)] @text.uri
"bullet" @punctuation.special

(strong) @text.strong
(emphasis) @text.emphasis
(literal) @text.literal

(list_item
  (term) @text.strong
  (classifier)? @text.emphasis)

(directive
  [".." (type) "::"] @function)

(field
  [":" (field_name) ":"] @property)

(interpreted_text) @text.literal
(interpreted_text (role) @keyword)
