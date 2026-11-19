; Repository map tags for Solidity, written for Cyber.
(contract_declaration name: (identifier) @name) @definition.class
(interface_declaration name: (identifier) @name) @definition.interface
(library_declaration name: (identifier) @name) @definition.module
(struct_declaration name: (identifier) @name) @definition.struct
(enum_declaration name: (identifier) @name) @definition.enum
(function_definition name: (identifier) @name) @definition.function
(modifier_definition name: (identifier) @name) @definition.function
(event_definition name: (identifier) @name) @definition.type
(error_declaration name: (identifier) @name) @definition.type
(user_defined_type_definition name: (identifier) @name) @definition.type
; References: calls, emitted events, modifiers, bases and types.
(call_expression function: (expression (identifier) @name)) @reference.call
(call_expression function: (expression (member_expression property: (identifier) @name))) @reference.call
(emit_statement name: (expression (identifier) @name)) @reference.event
(modifier_invocation (identifier) @name) @reference.call
(user_defined_type (identifier) @name) @reference.type
