import type { Integrations } from "./integrations.js";
const node = <K extends keyof HTMLElementTagNameMap>(
  tag: K,
  text = "",
): HTMLElementTagNameMap[K] => {
  const item = document.createElement(tag);
  item.textContent = text;
  return item;
};
function button(
  title: string,
  action: () => void,
  disabled: boolean,
): HTMLButtonElement {
  const item = node("button", title);
  item.type = "button";
  item.className = "button";
  item.disabled = disabled;
  item.addEventListener("click", action);
  return item;
}
function field(
  label: string,
  value: string,
  key: string,
  update: (value: string) => void,
): HTMLElement {
  const wrap = node("label"),
    input = node("textarea");
  wrap.className = "field";
  input.value = value;
  input.dataset.focus = key;
  input.setAttribute("aria-label", label);
  input.addEventListener("input", () => update(input.value));
  wrap.append(node("span", label), input);
  return wrap;
}
/** Patches, logs and contributor content are inert text. */
export function integrationsView(
  model: Integrations,
  enabled: boolean,
): HTMLElement {
  const section = node("section"),
    disabled = !enabled || model.busy || model.uncertain;
  section.append(
    node("h2", "Verified integration queue"),
    node(
      "p",
      "Combine local commits in an isolated checkout. Host checks verify the combined source; an independent read-only reviewer advises. Publication requires your review.",
    ),
    button(
      "Refresh queue",
      () => {
        void model.load();
      },
      disabled,
    ),
    field(
      "Integration manifest (JSON)",
      model.manifest,
      "integration-manifest",
      (value) => {
        model.manifest = value;
      },
    ),
    button(
      "Submit integration",
      () => {
        void model.submit();
      },
      disabled,
    ),
  );
  if (model.pendingTarget)
    section.append(node("p", `Original action: ${model.pendingTarget}`));
  if (model.uncertain)
    section.append(
      button(
        "Retry original integration action",
        () => {
          void model.retry();
        },
        !enabled || model.busy,
      ),
      button(
        "Refresh queue and clear local retry",
        () => {
          void model.discardRetry();
        },
        !enabled || model.busy,
      ),
    );
  if (model.notice) {
    const notice = node("p", model.notice);
    notice.setAttribute("role", "status");
    section.append(notice);
  }
  for (const batch of model.rows)
    section.append(
      button(
        `${batch.id} · ${batch.status} · revision ${batch.revision}`,
        () => {
          void model.load(batch.id);
        },
        disabled,
      ),
    );
  const batch = model.selected;
  if (!batch) return section;
  const evidence = node(
    "pre",
    JSON.stringify(
      {
        manifest: batch.manifest,
        report: batch.report,
        publication: batch.publication,
      },
      null,
      2,
    ),
  );
  evidence.className = "code";
  evidence.tabIndex = 0;
  section.append(
    node("h3", `${batch.id} · ${batch.status} · revision ${batch.revision}`),
    button(
      "Refresh selected evidence",
      () => {
        void model.load(batch.id);
      },
      disabled,
    ),
    evidence,
    button(
      "Run a new attempt",
      () => {
        void model.execute("run");
      },
      disabled ||
        (!!batch.run_id &&
          ["queued", "preparing", "checking", "reviewing"].includes(
            batch.status,
          )),
    ),
    button(
      "Cancel integration",
      () => {
        void model.execute("cancel");
      },
      disabled || batch.status === "cancelled",
    ),
  );
  if (batch.status === "ready") section.append(publication(model, disabled));
  return section;
}
function publication(model: Integrations, disabled: boolean): HTMLElement {
  const wrap = node("section");
  wrap.append(
    node("h3", "Publish a branch and pull request"),
    node(
      "p",
      "Review the exact manifest, combined patch, checks, reviewer limitations and cost above. Publication pushes the integration branch and opens a PR; it never merges.",
    ),
  );
  for (const key of ["remote", "repository", "title", "body"] as const)
    wrap.append(
      field(
        `Publication ${key}`,
        model.publication[key],
        `integration-${key}`,
        (value) => {
          model.publication[key] = value;
          model.reviewed = false;
          model.changed();
        },
      ),
    );
  const acknowledged = node("input");
  acknowledged.type = "checkbox";
  acknowledged.checked = model.publication.acknowledge_review_unavailable;
  acknowledged.addEventListener("change", () => {
    model.publication.acknowledge_review_unavailable = acknowledged.checked;
  });
  const unavailable = node(
    "label",
    "Acknowledge reviewer unavailable, if reported",
  );
  unavailable.prepend(acknowledged);
  wrap.append(unavailable);
  const check = node("input");
  check.type = "checkbox";
  check.checked = model.reviewed;
  const publish = button(
    "Publish reviewed integration",
    () => {
      void model.execute("publish");
    },
    disabled || !model.reviewed,
  );
  check.addEventListener("change", () => {
    model.reviewed = check.checked;
    publish.disabled = disabled || !check.checked;
  });
  const label = node("label", "I reviewed this exact revision and destination");
  label.prepend(check);
  wrap.append(label, publish);
  return wrap;
}
