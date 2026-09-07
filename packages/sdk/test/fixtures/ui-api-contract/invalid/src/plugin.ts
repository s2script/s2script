import { CustomHudLayout, type Modal, type ModalView, type UiErrorCode, type UiResult } from "@s2script/cs2";

declare const view: ModalView;

const invalidCode: UiErrorCode = "NoSuchUiError";
const wrongVoid: UiResult<void> = { ok: true, value: 1 };
const wrongView: UiResult<Modal> = { ok: true, value: view };
const typedLayout = CustomHudLayout.create({ addons: ["1"],
  resource: "panorama/layout/custom_game/typed.xml", buttons: ["save", "close"] });
typedLayout.subscribeClick("saev", () => {});

void invalidCode;
void wrongVoid;
void wrongView;

const legacyTypedLayout = CustomHudLayout.hud({ addons: ["1"],
  resource: "panorama/layout/custom_game/legacy-typed.xml", buttons: ["save", "close"] });
legacyTypedLayout.subscribeClick("saev", () => {});
