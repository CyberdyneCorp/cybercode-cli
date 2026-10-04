/**
 * A Mustache-subset template engine. See SPEC.md for the full contract.
 *
 * The template is parsed into a tree of nodes (text, variable, section, inverted, partial),
 * standalone lines are stripped while parsing, and the tree is rendered against a context
 * stack.
 */

const DEFAULT_DELIMITERS = ["{{", "}}"];
const STANDALONE_TYPES = new Set(["#", "^", "/", "!", ">", "="]);
const NAMED_TYPES = new Set(["name", "&", "#", "^", "/", ">"]);
const TRAILING_BLANK = /[ \t]*(\r?\n|$)/y;
const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

/**
 * @param {string} template
 * @param {unknown} view
 * @param {Record<string, string>} [partials]
 * @returns {string}
 */
export function render(template, view, partials = {}) {
  return renderNodes(parse(template), [view], partials);
}

class Parser {
  constructor(template) {
    this.template = template;
    this.pos = 0;
    this.line = 1;
    this.lineCountedTo = 0;
    [this.open, this.close] = DEFAULT_DELIMITERS;
    this.root = [];
    this.sections = [];
    this.text = "";
  }

  get output() {
    return this.sections.length ? this.sections.at(-1).children : this.root;
  }

  lineAt(index) {
    for (let i = this.lineCountedTo; i < index; i++) {
      if (this.template[i] === "\n") this.line++;
    }
    this.lineCountedTo = index;
    return this.line;
  }

  flushText() {
    if (this.text) this.output.push({ type: "text", value: this.text });
    this.text = "";
  }

  parse() {
    const { template } = this;
    while (this.pos < template.length) {
      const start = template.indexOf(this.open, this.pos);
      if (start === -1) {
        this.text += template.slice(this.pos);
        break;
      }
      this.text += template.slice(this.pos, start);
      this.readTag(start);
    }
    this.flushText();
    if (this.sections.length) {
      const open = this.sections.at(-1);
      throw new Error(`Unclosed section "${open.name}" at line ${open.line}`);
    }
    return this.root;
  }

  readTag(start) {
    const { template } = this;
    const line = this.lineAt(start);
    let contentStart = start + this.open.length;
    let closer = this.close;
    const triple = this.open === "{{" && this.close === "}}" && template[contentStart] === "{";
    if (triple) {
      contentStart++;
      closer = "}}}";
    }
    const end = template.indexOf(closer, contentStart);
    if (end === -1) throw new Error(`Unclosed tag at line ${line}`);
    const tag = { ...classify(template.slice(contentStart, end).trim(), triple), line };
    if (NAMED_TYPES.has(tag.type) && tag.name === "") throw new Error(`Empty tag at line ${line}`);

    this.pos = end + closer.length;
    tag.indent = "";
    if (STANDALONE_TYPES.has(tag.type)) this.stripStandalone(start, tag);
    this.addTag(tag);
  }

  /** Removes the tag's line when it is standalone, remembering its indentation. */
  stripStandalone(start, tag) {
    const lineStart = this.template.lastIndexOf("\n", start - 1) + 1;
    const indent = this.template.slice(lineStart, start);
    if (!/^[ \t]*$/.test(indent)) return;
    TRAILING_BLANK.lastIndex = this.pos;
    const trailing = TRAILING_BLANK.exec(this.template);
    if (!trailing) return;
    this.text = this.text.slice(0, this.text.length - indent.length);
    this.pos += trailing[0].length;
    tag.indent = indent;
  }

  addTag(tag) {
    switch (tag.type) {
      case "!":
        return;
      case "=":
        this.setDelimiters(tag);
        return;
      case "/":
        this.closeSection(tag);
        return;
    }
    this.flushText();
    const node = { type: tag.type, name: tag.name, line: tag.line, indent: tag.indent };
    this.output.push(node);
    if (tag.type === "#" || tag.type === "^") {
      node.children = [];
      this.sections.push(node);
    }
  }

  setDelimiters({ content, line }) {
    const parts = content.length >= 2 && content.endsWith("=")
      ? content.slice(1, -1).trim().split(/\s+/)
      : [];
    if (parts.length !== 2 || parts.some((part) => part === "" || part.includes("="))) {
      throw new Error(`Invalid delimiters at line ${line}`);
    }
    [this.open, this.close] = parts;
  }

  closeSection({ name, line }) {
    const open = this.sections.at(-1);
    if (!open) throw new Error(`Unexpected closing tag "${name}" at line ${line}`);
    if (open.name !== name) {
      throw new Error(
        `Closing tag "${name}" at line ${line} does not match "${open.name}" opened at line ${open.line}`,
      );
    }
    this.flushText();
    this.sections.pop();
  }
}

function classify(content, triple) {
  if (triple) return { type: "&", name: content };
  const sigil = content[0];
  if (sigil === "=") return { type: "=", content };
  if (sigil !== undefined && "#^/!>&".includes(sigil)) {
    return { type: sigil, name: content.slice(1).trim() };
  }
  return { type: "name", name: content };
}

function parse(template) {
  return new Parser(template).parse();
}

function renderNodes(nodes, stack, partials) {
  let out = "";
  for (const node of nodes) out += renderNode(node, stack, partials);
  return out;
}

function renderNode(node, stack, partials) {
  switch (node.type) {
    case "text":
      return node.value;
    case "name":
      return escapeHtml(toText(lookup(stack, node.name)));
    case "&":
      return toText(lookup(stack, node.name));
    case "#":
      return renderSection(node, stack, partials);
    case "^":
      return isTruthy(lookup(stack, node.name)) ? "" : renderNodes(node.children, stack, partials);
    case ">":
      return renderPartial(node, stack, partials);
  }
  throw new Error(`unknown node type ${node.type}`);
}

function renderSection(node, stack, partials) {
  const value = lookup(stack, node.name);
  if (!isTruthy(value)) return "";
  const items = Array.isArray(value) ? value : [value];
  return items.map((item) => renderNodes(node.children, [...stack, item], partials)).join("");
}

function renderPartial(node, stack, partials) {
  if (!Object.hasOwn(partials, node.name)) return "";
  const source = partials[node.name];
  if (typeof source !== "string") return "";
  return renderNodes(parse(indent(source, node.indent)), stack, partials);
}

function indent(source, prefix) {
  if (!prefix || !source) return source;
  return prefix + source.replace(/\n(?!$)/g, `\n${prefix}`);
}

function hasKey(frame, key) {
  return frame !== null && typeof frame === "object" && Object.hasOwn(frame, key);
}

function lookup(stack, name) {
  let value;
  if (name === ".") {
    value = stack.at(-1);
  } else {
    const [first, ...rest] = name.split(".");
    const frame = stack.findLast((candidate) => hasKey(candidate, first));
    if (frame === undefined) return undefined;
    value = frame[first];
    for (const segment of rest) {
      if (!hasKey(value, segment)) return undefined;
      value = value[segment];
    }
  }
  return typeof value === "function" ? undefined : value;
}

function isTruthy(value) {
  if (value === undefined || value === null || value === false || value === "") return false;
  return !(Array.isArray(value) && value.length === 0);
}

function toText(value) {
  return value === undefined || value === null ? "" : String(value);
}

function escapeHtml(text) {
  return text.replace(/[&<>"']/g, (char) => ESCAPES[char]);
}
