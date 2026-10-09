import type { TuiPlugin } from "@opencode-ai/plugin/tui";

// Acceptance probe: runs in an isolated OpenCode TUI without provider credentials.
export const tui: TuiPlugin = async (api) => {
  if (!api.slots?.register || !api.ui?.toast || !api.lifecycle?.onDispose) {
    throw new Error("Required public TUI APIs unavailable");
  }
  api.ui.toast({
    title: "AI Quota",
    message: "AI_QUOTA_TUI_API_OK",
    duration: 30000,
  });
};
export default { id: "ai-quota-minimal", tui };
