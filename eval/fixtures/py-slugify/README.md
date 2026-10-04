# py-slugify fixture

`slugify("Hello,  World!")` should return `hello-world`: runs of separators collapse into one
hyphen and leading or trailing hyphens are removed. The current implementation does neither.
Run the visible tests with `python3 -m unittest`.
