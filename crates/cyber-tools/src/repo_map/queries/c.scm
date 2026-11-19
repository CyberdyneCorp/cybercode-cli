; Repository map tags for C. Adapted from the tree-sitter-c tags.scm (MIT).
(function_definition
  declarator: (function_declarator declarator: (identifier) @name)) @definition.function
(function_definition
  declarator: (pointer_declarator
    declarator: (function_declarator declarator: (identifier) @name))) @definition.function
(declaration
  declarator: (function_declarator declarator: (identifier) @name)) @definition.function
(struct_specifier name: (type_identifier) @name body: (_)) @definition.struct
(union_specifier name: (type_identifier) @name body: (_)) @definition.struct
(enum_specifier name: (type_identifier) @name body: (_)) @definition.enum
(type_definition declarator: (type_identifier) @name) @definition.type
(preproc_function_def name: (identifier) @name) @definition.macro
(preproc_def name: (identifier) @name) @definition.macro
; References: calls and types.
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call
(type_identifier) @name @reference.type
