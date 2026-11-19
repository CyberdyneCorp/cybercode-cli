; Repository map tags for Java. Adapted from the tree-sitter-java tags.scm (MIT).
(class_declaration name: (identifier) @name) @definition.class
(record_declaration name: (identifier) @name) @definition.class
(interface_declaration name: (identifier) @name) @definition.interface
(enum_declaration name: (identifier) @name) @definition.enum
(annotation_type_declaration name: (identifier) @name) @definition.interface
(method_declaration name: (identifier) @name) @definition.method
(constructor_declaration name: (identifier) @name) @definition.method
; References: calls, constructors and types.
(method_invocation name: (identifier) @name) @reference.call
(type_identifier) @name @reference.type
