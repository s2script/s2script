import type { Modal, ModalView, UiErrorCode, UiResult } from "@s2script/cs2";

declare const view: ModalView;

const invalidCode: UiErrorCode = "NoSuchUiError";
const wrongVoid: UiResult<void> = { ok: true, value: 1 };
const wrongView: UiResult<Modal> = { ok: true, value: view };

void invalidCode;
void wrongVoid;
void wrongView;

