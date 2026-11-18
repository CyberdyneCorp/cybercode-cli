; Repository map tags for JavaScript and JSX. Adapted from the tree-sitter-javascript
; tags.scm (MIT).
(class_declaration name: (_) @name) @definition.class
(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(method_definition name: (_) @name) @definition.method
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
(class_heritage (identifier) @name) @reference.class

(jsx_opening_element name: (identifier) @name) @reference.class
(jsx_self_closing_element name: (identifier) @name) @reference.class
