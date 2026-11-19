; Repository map tags for Go. Adapted from the tree-sitter-go tags.scm (MIT).
(function_declaration name: (identifier) @name) @definition.function
(method_declaration name: (field_identifier) @name) @definition.method
(type_spec name: (type_identifier) @name type: (struct_type)) @definition.struct
(type_spec name: (type_identifier) @name type: (interface_type)) @definition.interface
(type_spec name: (type_identifier) @name) @definition.type
(const_spec name: (identifier) @name) @definition.constant
; References: calls, selectors and types.
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (selector_expression field: (field_identifier) @name)) @reference.call
(type_identifier) @name @reference.type
