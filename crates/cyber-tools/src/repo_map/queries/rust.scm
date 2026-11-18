; Repository map tags for Rust. Adapted from the tree-sitter-rust tags.scm (MIT).
; Captures: @name with @definition.<kind>, and @container for scopes that define nothing.
(struct_item name: (type_identifier) @name) @definition.struct
(union_item name: (type_identifier) @name) @definition.struct
(enum_item name: (type_identifier) @name) @definition.enum
(type_item name: (type_identifier) @name) @definition.type
(trait_item name: (type_identifier) @name) @definition.interface
(function_item name: (identifier) @name) @definition.function
(function_signature_item name: (identifier) @name) @definition.function
(mod_item name: (identifier) @name body: (_)) @definition.module
(macro_definition name: (identifier) @name) @definition.macro
(const_item name: (identifier) @name) @definition.constant
(static_item name: (identifier) @name) @definition.constant
(impl_item) @container
; References: calls, macros, paths and types (not local variables).
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call
(call_expression function: (scoped_identifier name: (identifier) @name)) @reference.call
(macro_invocation macro: (identifier) @name) @reference.call
(scoped_identifier name: (identifier) @name) @reference.path
(type_identifier) @name @reference.type
