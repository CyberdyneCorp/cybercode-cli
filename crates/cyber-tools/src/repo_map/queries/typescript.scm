; Repository map tags for TypeScript and TSX. Adapted from the tree-sitter-typescript and
; tree-sitter-javascript tags.scm (MIT).
(class_declaration name: (_) @name) @definition.class
(abstract_class_declaration name: (_) @name) @definition.class
(interface_declaration name: (_) @name) @definition.interface
(type_alias_declaration name: (_) @name) @definition.type
(enum_declaration name: (_) @name) @definition.enum
(internal_module name: (_) @name) @definition.module
(module name: (_) @name) @definition.module
(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(function_signature name: (identifier) @name) @definition.function
(method_definition name: (_) @name) @definition.method
(method_signature name: (_) @name) @definition.method
(abstract_method_signature name: (_) @name) @definition.method
(program
  (lexical_declaration
    (variable_declarator
      name: (identifier) @name
      value: [(arrow_function) (function_expression)])) @definition.function)
(export_statement
  (lexical_declaration
    (variable_declarator
      name: (identifier) @name
      value: [(arrow_function) (function_expression)])) @definition.function)
; References: calls, constructors, JSX components and imported names.
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call
(new_expression constructor: (identifier) @name) @reference.class
(import_specifier name: (identifier) @name) @reference.import
(extends_clause value: (identifier) @name) @reference.class
(type_identifier) @name @reference.type
