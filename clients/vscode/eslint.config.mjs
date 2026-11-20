// Lint: TypeScript rules plus a cognitive complexity limit of 15 per function.
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import sonarjs from "eslint-plugin-sonarjs";

export default tseslint.config(
  { ignores: ["dist/**", "out/**", "node_modules/**", ".vscode-test/**"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    plugins: { sonarjs },
    languageOptions: { globals: { console: "readonly", process: "readonly", __dirname: "readonly" } },
    rules: {
      "sonarjs/cognitive-complexity": ["error", 15],
      "no-restricted-syntax": ["error", { selector: "CallExpression[callee.property.name='innerHTML']", message: "Render text nodes only." }, { selector: "AssignmentExpression[left.property.name='innerHTML']", message: "Render text nodes only." }],
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_" }],
    },
  },
);
