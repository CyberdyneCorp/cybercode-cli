; Repository map tags for Python. Adapted from the tree-sitter-python tags.scm (MIT).
(class_definition name: (identifier) @name) @definition.class
(function_definition name: (identifier) @name) @definition.function
(module
  (expression_statement
    (assignment left: (identifier) @name) @definition.constant)
  (#match? @name "^[A-Z][A-Z0-9_]*$"))
; References: calls, base classes, annotations and imported names.
(call function: (identifier) @name) @reference.call
(call function: (attribute attribute: (identifier) @name)) @reference.call
(class_definition superclasses: (argument_list (identifier) @name)) @reference.class
(type (identifier) @name) @reference.type
(import_from_statement name: (dotted_name (identifier) @name)) @reference.import
