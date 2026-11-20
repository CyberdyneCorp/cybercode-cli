/**
 * Auto-start consent (`editor-integration` → Extension server auto-start consent): what the
 * machine-scoped `cyber.autoStartServer` and the one-time modal decide.
 */

export const QUESTION = "Start the Cyber Code background server? It keeps running after VS Code closes.";
export const CHOICES = ["Always start", "Start this time", "Don't start"] as const;
export type Choice = (typeof CHOICES)[number];

/** `start` now, `ask` with the modal, or `wait` for the Start server action. */
export function decide(setting: string | undefined, askedThisWindow: boolean): "start" | "ask" | "wait" {
  if (setting === "always") return "start";
  if (setting === "never" || askedThisWindow) return "wait";
  return "ask";
}

/** Whether to start, and the value to save in user settings, for the modal's answer. */
export function answer(choice: Choice | undefined): { start: boolean; save?: "always" | "never" } {
  if (choice === "Always start") return { start: true, save: "always" };
  if (choice === "Start this time") return { start: true };
  if (choice === "Don't start") return { start: false, save: "never" };
  return { start: false };
}
