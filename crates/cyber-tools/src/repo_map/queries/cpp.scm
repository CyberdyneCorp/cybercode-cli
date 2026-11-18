; Repository map tags for C++. Adapted from the tree-sitter-cpp and tree-sitter-c tags.scm
; (MIT).
(function_definition
  declarator: (function_declarator declarator: (identifier) @name)) @definition.function
(function_definition
  declarator: (function_declarator declarator: (field_identifier) @name)) @definition.method
(function_definition
  declarator: (function_declarator
    declarator: (qualified_identifier name: (identifier) @name))) @definition.method
(function_definition
  declarator: (pointer_declarator
    declarator: (function_declarator declarator: (identifier) @name))) @definition.function
(field_declaration
  declarator: (function_declarator declarator: (field_identifier) @name)) @definition.method
(declaration
  declarator: (function_declarator declarator: (identifier) @name)) @definition.function
(class_specifier name: (type_identifier) @name body: (_)) @definition.class
(struct_specifier name: (type_identifier) @name body: (_)) @definition.struct
(union_specifier name: (type_identifier) @name body: (_)) @definition.struct
(enum_specifier name: (type_identifier) @name body: (_)) @definition.enum
(type_definition declarator: (type_identifier) @name) @definition.type
(alias_declaration name: (type_identifier) @name) @definition.type
(namespace_definition name: (namespace_identifier) @name) @definition.module
(preproc_function_def name: (identifier) @name) @definition.macro
(preproc_def name: (identifier) @name) @definition.macro
; References: calls and types.
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call
(type_identifier) @name @reference.type
(call_expression function: (qualified_identifier name: (identifier) @name)) @reference.call
(namespace_identifier) @name @reference.module
