import {
  hudkit,
  type Badge,
  type Dashboard,
  type DashboardSpec,
  type DashboardView,
  type Modal,
  type ModalOpenResult,
  type ModalSpec,
  type ModalView,
  type UiErrorCode,
  type UiResult,
  CustomHudLayout,
  type UiSubscription,
  type UiSurfaceHandle,
} from "@s2script/cs2";

declare const modalSpec: ModalSpec;
declare const dashboardSpec: DashboardSpec;

function consumeResult<T>(result: UiResult<T>): T | undefined {
  if (result.ok) return result.value;
  const code: UiErrorCode = result.error.code;
  const message: string = result.error.message;
  console.log(code, message);
  return undefined;
}

export function OnPluginStart(): void {
  const layout = CustomHudLayout.create({
    addons: ["1"], resource: "panorama/layout/custom_game/typed.xml", buttons: ["save", "close"],
  });
  const subscription: UiSubscription = layout.subscribeClick("save", () => {});
  subscription.dispose();
  const surface: UiResult<UiSurfaceHandle> = hudkit.forSlot(1).tryOwnBanner({ text: "ready" });
  void surface;
  const modal: Modal | undefined = consumeResult(hudkit.tryModal(modalSpec));
  const badge: Badge | undefined = consumeResult(hudkit.tryBadge({ corner: "tr" }));

  if (modal) {
    const opened: ModalView | undefined = consumeResult(modal.tryOpenResult(1));
    const refreshed: void | undefined = consumeResult(modal.tryRefresh(1));
    if (opened) {
      const reopened: ModalView | undefined = consumeResult(opened.tryOpenResult({ cursor: false }));
      const viewRefresh: void | undefined = consumeResult(opened.tryRefresh());
      void reopened;
      void viewRefresh;
    }
    void refreshed;
  }

  const dashboard: Dashboard = hudkit.dashboard(dashboardSpec);
  const dashView: DashboardView | undefined = consumeResult(dashboard.tryOpenResult(1, { tab: "main" }));
  const dashRefresh: void | undefined = consumeResult(dashboard.tryRefresh(1));
  if (dashView) {
    const reopened: DashboardView | undefined = consumeResult(dashView.tryOpenResult({ cursor: false }));
    const viewRefresh: void | undefined = consumeResult(dashView.tryRefresh());
    void reopened;
    void viewRefresh;
  }
  void dashRefresh;

  if (badge) {
    const badgeView = badge.show(1, { text: "ready" });
    const shown: void | undefined = consumeResult(badgeView.tryShow({ text: "still ready" }));
    void shown;
  }

  // Existing APIs retain their published signatures while structured calls are adopted incrementally.
  const legacyModal: Modal | null = hudkit.modal(modalSpec);
  if (legacyModal) {
    const legacyOpen: ModalOpenResult = legacyModal.tryOpen(1);
    legacyModal.refresh();
    void legacyOpen;
  }
}
