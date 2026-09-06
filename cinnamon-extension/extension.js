// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

const Gio = imports.gi.Gio;
const GLib = imports.gi.GLib;
const St = imports.gi.St;
const Clutter = imports.gi.Clutter;
const Main = imports.ui.main;

const GAZE_DBUS_INTERFACE = `
<node>
  <interface name="com.gundulabs.Gaze">
    <property name="PamInternal" type="as" access="read"/>
    <method name="AddPamInternal">
      <arg name="service" type="s" direction="in"/>
    </method>
    <method name="RemovePamInternal">
      <arg name="service" type="s" direction="in"/>
    </method>
    <method name="ClearPamInternal"/>
    <method name="RegisterExtension">
      <arg name="active" type="b" direction="in"/>
    </method>
    <method name="IsExtensionActive">
      <arg name="uid" type="u" direction="in"/>
      <arg name="active" type="b" direction="out"/>
    </method>
    <method name="HasEnrolledFaces">
      <arg name="username" type="s" direction="in"/>
      <arg name="result" type="b" direction="out"/>
    </method>
    <method name="IsCameraAvailable">
      <arg name="result" type="b" direction="out"/>
    </method>
  </interface>
</node>
`;
const GazeProxy = Gio.DBusProxy.makeProxyWrapper(GAZE_DBUS_INTERFACE);

const CONFIRMATION_QUESTION = "Face Verified. Press Enter to confirm.";
const CONFIRMATION_DIALOG_LABEL =
  "Face verified. Press Enter or click Authenticate to confirm.";

const GAZE_MSG_LOOK_CAMERA = "GAZE_MSG_LOOK_CAMERA";
const GAZE_MSG_LOOK_OR_PASSWORD = "GAZE_MSG_LOOK_OR_PASSWORD";
const GAZE_MSG_FACE_VERIFIED = "GAZE_MSG_FACE_VERIFIED";
const GAZE_REQUIRE_CONFIRMATION = "GAZE_REQUIRE_CONFIRMATION";
const GAZE_CONFIRMED = "GAZE_CONFIRMED";
const GAZE_CANCEL = "GAZE_CANCEL";
const GAZE_MSG_FACE_NOT_RECOGNIZED = "GAZE_MSG_FACE_NOT_RECOGNIZED";
const GAZE_MSG_FACE_NOT_DETECTED = "GAZE_MSG_FACE_NOT_DETECTED";
const GAZE_MSG_FACE_TOO_DARK = "GAZE_MSG_FACE_TOO_DARK";
const GAZE_MSG_FACE_TIMED_OUT = "GAZE_MSG_FACE_TIMED_OUT";
const GAZE_MSG_FACE_UNAVAILABLE = "GAZE_MSG_FACE_UNAVAILABLE";

const CINNAMON_PAM_SERVICES = ["polkit-1", "cinnamon"];

const isConfirmationMessage = (text) => {
  const trimmed = text?.trim();
  return (
    trimmed === GAZE_REQUIRE_CONFIRMATION ||
    trimmed === CONFIRMATION_QUESTION
  );
};

const INTERNAL_ERROR_MAP = new Map([
  [
    GAZE_MSG_FACE_NOT_RECOGNIZED,
    "Face not recognized. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_NOT_DETECTED,
    "Face not detected. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_TOO_DARK,
    "Too dark for face authentication. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_TIMED_OUT,
    "Face authentication timed out. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_UNAVAILABLE,
    "Face authentication unavailable. Please enter your password.",
  ],
]);

const GENERIC_ERROR_MAP = new Map([
  [
    "Sorry, that did not work. Please try again.",
    "Sorry, face authentication did not work. Please try again.",
  ],
  [
    "Sorry, that didn\u2019t work. Please try again.",
    "Sorry, face authentication did not work. Please try again.",
  ],
  [
    "You reached the maximum authentication attempts, please try another method",
    "You reached the maximum face authentication attempts, please try another method",
  ],
]);

const FACE_STATUS_UPDATES = new Set([
  "Please look at the camera...",
  "Need more light...",
  "Face is clipped. Please move back...",
  "Please center your face...",
  "Please come closer...",
  "Please back up...",
  "Hold still...",
]);


const cancelDelayedReset = (dialog) => {
  if (!dialog._sessionRequestTimeoutId) return;
  GLib.source_remove(dialog._sessionRequestTimeoutId);
  dialog._sessionRequestTimeoutId = 0;
};

const keepPasswordEntryVisible = (dialog) => {
  const entry = dialog._passwordEntry;
  if (!entry || !dialog._session) return;
  if (!entry.hint_text) entry.hint_text = "Password";
  entry.show();
  entry.reactive = true;
  cancelDelayedReset(dialog);
};

const enterConfirmMode = (dialog, isInternal = true) => {
  cancelDelayedReset(dialog);
  dialog._isInternalConfirm = isInternal;
  dialog._passwordEntry?.set_text("");
  if (dialog._passwordEntry) {
    dialog._passwordEntry.reactive = false;
    dialog._passwordEntry.hide();
  }

  if (dialog._infoMessageLabel) {
    dialog._infoMessageLabel.text = CONFIRMATION_DIALOG_LABEL;
    dialog._infoMessageLabel.show();
  }
  dialog._errorMessageLabel?.hide();
  dialog._nullMessageLabel?.hide();

  if (dialog._okButton) {
    dialog._okButton.reactive = true;
    dialog._okButton.track_hover = true;
  }

  dialog._confirmMode = true;
  dialog._ensureOpen();
  dialog._okButton?.grab_key_focus();
};

const respondToConfirm = (dialog) => {
  dialog._confirmMode = false;
  const answer = dialog._isInternalConfirm !== false ? GAZE_CONFIRMED : "";
  dialog._session?.response(answer);
  dialog._passwordEntry?.set_text("");
  if (dialog._passwordEntry) dialog._passwordEntry.reactive = false;
  if (dialog._okButton) dialog._okButton.reactive = false;
};


const ensureUnlockConfirmButton = (unlockDialog, extension) => {
  if (unlockDialog._confirmButton) return unlockDialog._confirmButton;

  const button = new St.Button({
    style_class: "button modal-dialog-button default",
    style: "padding: 9px 24px; border-radius: 12px;",
    label: "Confirm Face Unlock",
    can_focus: true,
    reactive: true,
    x_align: Clutter.ActorAlign.FILL,
    x_expand: true,
    y_align: Clutter.ActorAlign.CENTER,
    visible: false,
  });

  button.connect("clicked", () => {
    if (unlockDialog._confirmMode) {
      respondToUnlockConfirm(unlockDialog);
    }
  });

  unlockDialog._confirmButton = button;
  if (extension?._activeConfirmWidgets) {
    extension._activeConfirmWidgets.add(button);
  }

  const spinnerBin = new St.Bin({
    x_align: Clutter.ActorAlign.CENTER,
    y_align: Clutter.ActorAlign.CENTER,
    x_expand: true,
    visible: false,
  });

  let spinner = null;
  try {
    const Animation = imports.ui.animation;
    if (Animation?.Spinner) {
      spinner = new Animation.Spinner(16);
    }
  } catch (e) {}

  if (!spinner) {
    spinner = new St.Icon({
      icon_name: "process-working-symbolic",
      icon_type: St.IconType.SYMBOLIC,
      icon_size: 16,
    });
  }

  spinnerBin.set_child(spinner);
  unlockDialog._confirmSpinnerBin = spinnerBin;
  unlockDialog._confirmSpinner = spinner;
  if (extension?._activeConfirmWidgets) {
    extension._activeConfirmWidgets.add(spinnerBin);
  }

  const targetBox =
    unlockDialog._contentLayout || unlockDialog._dialogBox || unlockDialog;
  targetBox.add_child(button);
  targetBox.add_child(spinnerBin);

  return button;
};

const enterUnlockConfirmMode = (unlockDialog, extension, isInternal = true) => {
  try {
    unlockDialog._confirmMode = true;
    unlockDialog._isInternalConfirm = isInternal;

    if (unlockDialog._passwordEntry) {
      unlockDialog._passwordEntry.set_text("");
      unlockDialog._passwordEntry.hide();
      unlockDialog._passwordEntry.reactive = false;
    }
    if (unlockDialog._capsLockWarning) {
      unlockDialog._capsLockWarning.hide();
    }
    if (unlockDialog._messageLabel) {
      unlockDialog._messageLabel.text = "";
    }
    if (unlockDialog._infoLabel) {
      unlockDialog._infoLabel.text = "";
    }

    if (unlockDialog._confirmSpinner?.stop) {
      unlockDialog._confirmSpinner.stop();
    }
    if (unlockDialog._confirmSpinnerBin) {
      unlockDialog._confirmSpinnerBin.hide();
    }

    const button = ensureUnlockConfirmButton(unlockDialog, extension);
    button.show();
    button.reactive = true;
    button.can_focus = true;
    button.grab_key_focus();
  } catch (e) {
    global.logError("[gaze] Failed to enter unlock confirm mode", e);
    exitUnlockConfirmMode(unlockDialog);
  }
};

const respondToUnlockConfirm = (unlockDialog) => {
  if (!unlockDialog._confirmMode) return;
  unlockDialog._confirmMode = false;

  if (unlockDialog._confirmButton) {
    unlockDialog._confirmButton.reactive = false;
    unlockDialog._confirmButton.hide();
  }

  if (unlockDialog._confirmSpinnerBin) {
    unlockDialog._confirmSpinnerBin.show();
    if (unlockDialog._confirmSpinner?.play) {
      unlockDialog._confirmSpinner.play();
    }
  }

  if (typeof unlockDialog._setBusy === "function") {
    unlockDialog._setBusy(true);
  }

  if (
    unlockDialog._authClient &&
    typeof unlockDialog._authClient.sendPassword === "function"
  ) {
    const answer = unlockDialog._isInternalConfirm !== false ? GAZE_CONFIRMED : "";
    unlockDialog._authClient.sendPassword(answer);
  }
};


const exitUnlockConfirmMode = (unlockDialog) => {
  if (
    !unlockDialog._confirmMode &&
    !unlockDialog._confirmButton?.visible &&
    !unlockDialog._confirmSpinnerBin?.visible
  )
    return;

  unlockDialog._confirmMode = false;

  if (unlockDialog._confirmSpinner?.stop) {
    unlockDialog._confirmSpinner.stop();
  }
  if (unlockDialog._confirmSpinnerBin) {
    unlockDialog._confirmSpinnerBin.hide();
  }
  if (unlockDialog._confirmButton) {
    unlockDialog._confirmButton.reactive = false;
    unlockDialog._confirmButton.hide();
  }

  if (unlockDialog._passwordEntry) {
    unlockDialog._passwordEntry.show();
    unlockDialog._passwordEntry.reactive = true;
    unlockDialog._passwordEntry.set_text("");
  }
};

class GazeCinnamonExtension {
  constructor(metadata) {
    this._metadata = metadata;
    this._uuid = metadata?.uuid || "gaze@gundulabs.com";
    this._dbusProxy = null;
    this._nameOwnerId = 0;
    this._activeConfirmWidgets = new Set();
    this._polkitPatched = false;
    this._unlockPatched = false;
    this._settings = null;
  }

  getFaceEnabled() {
    try {
      if (this._settings) {
        return Boolean(this._settings.getValue("enable-face-authentication"));
      }
    } catch (e) {}
    return true;
  }

  getMaxTries() {
    try {
      if (this._settings) {
        const val = this._settings.getValue("max-face-tries");
        return Math.max(2, Number(val) || 3);
      }
    } catch (e) {}
    return 3;
  }

  getRetryMode() {
    try {
      if (this._settings) {
        return this._settings.getValue("face-retry-mode") || "fixed";
      }
    } catch (e) {}
    return "fixed";
  }

  canFaceRetry(dialog) {
    const mode = this.getRetryMode();
    if (mode === "disabled") {
      return (dialog._faceFailCounter ?? 0) < 1;
    } else if (mode === "infinite") {
      return true;
    } else {
      return (dialog._faceFailCounter ?? 0) < this.getMaxTries();
    }
  }

  enable() {
    this._activeConfirmWidgets = new Set();

    try {
      const Settings = imports.ui.settings;
      if (Settings?.ExtensionSettings) {
        this._settings = new Settings.ExtensionSettings(this, this._uuid);
      }
    } catch (e) {
      global.logWarning("[gaze] Failed to initialize ExtensionSettings", e);
    }

    try {
      this._dbusProxy = new GazeProxy(
        Gio.DBus.system,
        "com.gundulabs.Gaze",
        "/com/gundulabs/Gaze",
        (proxy, error) => {
          if (error) return;
          try {
            proxy.RegisterExtensionRemote(true);
            for (const service of CINNAMON_PAM_SERVICES) {
              proxy.AddPamInternalRemote(service);
            }
          } catch (e) {}
        },
      );

      this._nameOwnerId = this._dbusProxy.connect("notify::g-name-owner", () => {
        if (this._dbusProxy?.g_name_owner) {
          try {
            this._dbusProxy.RegisterExtensionRemote(true);
            for (const service of CINNAMON_PAM_SERVICES) {
              this._dbusProxy.AddPamInternalRemote(service);
            }
          } catch (e) {}
        }
      });
    } catch (e) {
      global.logError("[gaze] Failed to initialize D-Bus proxy", e);
    }

    this._patchPolkit();
    this._patchUnlockDialog();
  }

  _patchPolkit() {
    let PolkitAgent = null;
    try {
      PolkitAgent = imports.ui.polkitAuthenticationAgent;
    } catch (e) {
      global.logWarning("[gaze] Cinnamon polkitAuthenticationAgent not found", e);
      return;
    }

    if (!PolkitAgent?.AuthenticationAgent || !PolkitAgent?.AuthenticationDialog)
      return;

    const agentProto = PolkitAgent.AuthenticationAgent.prototype;
    const dialogProto = PolkitAgent.AuthenticationDialog.prototype;
    const ext = this;

    if (!agentProto._gazeOriginalInitiate) {
      agentProto._gazeOriginalInitiate = agentProto._onInitiate;
      agentProto._onInitiate = function (
        nativeAgent,
        actionId,
        message,
        iconName,
        cookie,
        userNames,
      ) {
        if (ext._dbusProxy) {
          try {
            ext._dbusProxy.RegisterExtensionRemote(true);
            for (const service of CINNAMON_PAM_SERVICES) {
              ext._dbusProxy.AddPamInternalRemote(service);
            }
          } catch (e) {}
        }

        agentProto._gazeOriginalInitiate.call(
          this,
          nativeAgent,
          actionId,
          message,
          iconName,
          cookie,
          userNames,
        );

        const dialog = this._currentDialog;
        if (!dialog) return;

        keepPasswordEntryVisible(dialog);

        if (dialog._session) {
          dialog._session.connect("show-info", (session, text) => {
            const trimmed = text?.trim();
            if (isConfirmationMessage(trimmed)) {
              enterConfirmMode(dialog, trimmed === GAZE_REQUIRE_CONFIRMATION);
            }
          });
        }

        const origEntryActivate = dialog._onEntryActivate;
        dialog._onEntryActivate = function () {
          if (this._confirmMode) {
            respondToConfirm(this);
          } else {
            origEntryActivate.call(this);
          }
        };

        const origAuthButton = dialog._onAuthenticateButtonPressed;
        if (typeof origAuthButton === "function") {
          dialog._onAuthenticateButtonPressed = function () {
            if (this._confirmMode) {
              respondToConfirm(this);
            } else {
              origAuthButton.call(this);
            }
          };
        }
      };
    }

    if (!dialogProto._gazeOverridden) {
      dialogProto._gazeOverridden = true;

      dialogProto._gazeOriginalShowInfo = dialogProto._onSessionShowInfo;
      dialogProto._onSessionShowInfo = function (session, text) {
        const trimmed = text?.trim();
        if (isConfirmationMessage(trimmed)) {
          enterConfirmMode(this, trimmed === GAZE_REQUIRE_CONFIRMATION);
          return;
        }
        if (
          trimmed === GAZE_MSG_LOOK_CAMERA ||
          trimmed === GAZE_MSG_LOOK_OR_PASSWORD ||
          trimmed === GAZE_MSG_FACE_VERIFIED
        ) {
          return;
        }
        if (INTERNAL_ERROR_MAP.has(trimmed)) {
          const mapped = INTERNAL_ERROR_MAP.get(trimmed);
          dialogProto._gazeOriginalShowInfo.call(this, session, mapped);
          return;
        }
        dialogProto._gazeOriginalShowInfo.call(this, session, text);
      };

      dialogProto._gazeOriginalShowError = dialogProto._onSessionShowError;
      if (typeof dialogProto._gazeOriginalShowError === "function") {
        dialogProto._onSessionShowError = function (session, text) {
          const trimmed = text?.trim();
          if (INTERNAL_ERROR_MAP.has(trimmed)) {
            const mapped = INTERNAL_ERROR_MAP.get(trimmed);
            dialogProto._gazeOriginalShowError.call(this, session, mapped);
            return;
          }
          const mapped = GENERIC_ERROR_MAP.get(trimmed) ?? text;
          dialogProto._gazeOriginalShowError.call(this, session, mapped);
        };
      }


      dialogProto._gazeOriginalEntryActivate = dialogProto._onEntryActivate;
      dialogProto._onEntryActivate = function () {
        if (this._confirmMode) {
          respondToConfirm(this);
        } else {
          dialogProto._gazeOriginalEntryActivate.call(this);
        }
      };

      dialogProto._gazeOriginalAuthButton =
        dialogProto._onAuthenticateButtonPressed;
      if (typeof dialogProto._gazeOriginalAuthButton === "function") {
        dialogProto._onAuthenticateButtonPressed = function () {
          if (this._confirmMode) {
            respondToConfirm(this);
          } else {
            dialogProto._gazeOriginalAuthButton.call(this);
          }
        };
      }

      dialogProto._gazeOriginalDestroySession = dialogProto._destroySession;
      if (typeof dialogProto._gazeOriginalDestroySession === "function") {
        dialogProto._destroySession = function (delay) {
          dialogProto._gazeOriginalDestroySession.call(this, delay);
          if (!delay) return;
          cancelDelayedReset(this);
          if (this._passwordEntry) {
            this._passwordEntry.show();
            this._passwordEntry.reactive = true;
          }
        };
      }
    }

    this._polkitPatched = true;
  }

  _patchUnlockDialog() {
    let UnlockDialogClass = null;
    try {
      UnlockDialogClass = imports.ui.screensaver?.unlockDialog?.UnlockDialog;
    } catch (e) {}

    if (!UnlockDialogClass?.prototype) return;

    const proto = UnlockDialogClass.prototype;
    if (proto._gazeOverridden) return;
    proto._gazeOverridden = true;

    const ext = this;

    proto._gazeOriginalOnAuthInfo = proto._onAuthInfo;
    proto._onAuthInfo = function (authClient, info) {
      const trimmed = info?.trim();
      if (isConfirmationMessage(trimmed)) {
        enterUnlockConfirmMode(this, ext, trimmed === GAZE_REQUIRE_CONFIRMATION);
        return;
      }
      if (
        trimmed === GAZE_MSG_LOOK_CAMERA ||
        trimmed === GAZE_MSG_LOOK_OR_PASSWORD ||
        trimmed === GAZE_MSG_FACE_VERIFIED
      ) {
        return;
      }
      if (INTERNAL_ERROR_MAP.has(trimmed)) {
        this._infoLabel.text = INTERNAL_ERROR_MAP.get(trimmed);
        return;
      }
      if (trimmed && FACE_STATUS_UPDATES.has(trimmed)) {
        this._infoLabel.text = trimmed;
        return;
      }
      exitUnlockConfirmMode(this);
      proto._gazeOriginalOnAuthInfo.call(this, authClient, info);
    };

    proto._gazeOriginalOnAuthPrompt = proto._onAuthPrompt;
    proto._onAuthPrompt = function (authClient, prompt) {
      const trimmed = prompt?.trim();
      if (isConfirmationMessage(trimmed)) {
        enterUnlockConfirmMode(this, ext, trimmed === GAZE_REQUIRE_CONFIRMATION);
        return;
      }
      exitUnlockConfirmMode(this);
      proto._gazeOriginalOnAuthPrompt.call(this, authClient, prompt);
    };

    proto._gazeOriginalOnAuthError = proto._onAuthError;
    proto._onAuthError = function (authClient, error) {
      exitUnlockConfirmMode(this);
      const trimmed = error?.trim();
      const mapped =
        INTERNAL_ERROR_MAP.get(trimmed) ??
        GENERIC_ERROR_MAP.get(trimmed) ??
        error;
      proto._gazeOriginalOnAuthError.call(this, authClient, mapped);
    };

    proto._gazeOriginalOnAuthSuccess = proto._onAuthSuccess;
    proto._onAuthSuccess = function () {
      this._faceFailCounter = 0;
      this._faceAuthFailed = false;
      if (this._confirmButton) this._confirmButton.hide();
      if (this._confirmSpinnerBin) this._confirmSpinnerBin.hide();
      proto._gazeOriginalOnAuthSuccess.call(this);
    };

    proto._gazeOriginalOnAuthFailure = proto._onAuthFailure;
    proto._onAuthFailure = function () {
      exitUnlockConfirmMode(this);
      this._faceFailCounter = (this._faceFailCounter || 0) + 1;
      proto._gazeOriginalOnAuthFailure.call(this);

      if (ext.getFaceEnabled() && ext.canFaceRetry(this)) {
        try {
          this.initializePam();
        } catch (e) {}
      } else {
        this._faceAuthFailed = true;
      }
    };

    proto._gazeOriginalOnAuthCancel = proto._onAuthCancel;
    proto._onAuthCancel = function () {
      this._faceFailCounter = 0;
      this._faceAuthFailed = false;
      exitUnlockConfirmMode(this);
      proto._gazeOriginalOnAuthCancel.call(this);
    };

    proto._gazeOriginalOnUnlock = proto._onUnlock;
    proto._onUnlock = function () {
      if (this._confirmMode) {
        respondToUnlockConfirm(this);
        return;
      }
      proto._gazeOriginalOnUnlock.call(this);
    };

    proto._gazeOriginalOnKeyPress = proto._onKeyPress;
    proto._onKeyPress = function (actor, event) {
      const symbol = event.get_key_symbol();
      if (this._confirmMode) {
        if (
          symbol === Clutter.KEY_Return ||
          symbol === Clutter.KEY_KP_Enter ||
          symbol === Clutter.KEY_ISO_Enter
        ) {
          respondToUnlockConfirm(this);
          return Clutter.EVENT_STOP;
        }
        if (symbol === Clutter.KEY_Escape) {
          exitUnlockConfirmMode(this);
          this._onCancel();
          return Clutter.EVENT_STOP;
        }
      }
      return proto._gazeOriginalOnKeyPress.call(this, actor, event);
    };

    this._unlockPatched = true;
  }

  disable() {
    if (this._settings?.finalize) {
      try {
        this._settings.finalize();
      } catch (e) {}
      this._settings = null;
    }

    if (this._dbusProxy) {
      if (this._nameOwnerId) {
        this._dbusProxy.disconnect(this._nameOwnerId);
        this._nameOwnerId = 0;
      }
      try {
        for (const service of CINNAMON_PAM_SERVICES) {
          this._dbusProxy.RemovePamInternalRemote(service);
        }
        this._dbusProxy.RegisterExtensionRemote(false);
      } catch (e) {}
      this._dbusProxy = null;
    }

    if (this._activeConfirmWidgets) {
      for (const widget of this._activeConfirmWidgets) {
        try {
          widget.destroy();
        } catch (e) {}
      }
      this._activeConfirmWidgets.clear();
      this._activeConfirmWidgets = null;
    }

    if (this._polkitPatched) {
      try {
        const PolkitAgent = imports.ui.polkitAuthenticationAgent;
        if (PolkitAgent?.AuthenticationAgent?.prototype?._gazeOriginalInitiate) {
          PolkitAgent.AuthenticationAgent.prototype._onInitiate =
            PolkitAgent.AuthenticationAgent.prototype._gazeOriginalInitiate;
          delete PolkitAgent.AuthenticationAgent.prototype._gazeOriginalInitiate;
        }
        if (PolkitAgent?.AuthenticationDialog?.prototype?._gazeOverridden) {
          const p = PolkitAgent.AuthenticationDialog.prototype;
          if (p._gazeOriginalShowInfo) p._onSessionShowInfo = p._gazeOriginalShowInfo;
          if (p._gazeOriginalShowError)
            p._onSessionShowError = p._gazeOriginalShowError;
          if (p._gazeOriginalEntryActivate)
            p._onEntryActivate = p._gazeOriginalEntryActivate;
          if (p._gazeOriginalAuthButton)
            p._onAuthenticateButtonPressed = p._gazeOriginalAuthButton;
          if (p._gazeOriginalDestroySession)
            p._destroySession = p._gazeOriginalDestroySession;
          delete p._gazeOverridden;
          delete p._gazeOriginalShowInfo;
          delete p._gazeOriginalShowError;
          delete p._gazeOriginalEntryActivate;
          delete p._gazeOriginalAuthButton;
          delete p._gazeOriginalDestroySession;
        }
      } catch (e) {}
      this._polkitPatched = false;
    }


    if (this._unlockPatched) {
      try {
        const UnlockDialogClass =
          imports.ui.screensaver?.unlockDialog?.UnlockDialog;
        if (UnlockDialogClass?.prototype?._gazeOverridden) {
          const p = UnlockDialogClass.prototype;
          if (p._gazeOriginalOnAuthInfo) p._onAuthInfo = p._gazeOriginalOnAuthInfo;
          if (p._gazeOriginalOnAuthPrompt)
            p._onAuthPrompt = p._gazeOriginalOnAuthPrompt;
          if (p._gazeOriginalOnAuthError)
            p._onAuthError = p._gazeOriginalOnAuthError;
          if (p._gazeOriginalOnAuthSuccess)
            p._onAuthSuccess = p._gazeOriginalOnAuthSuccess;
          if (p._gazeOriginalOnAuthFailure)
            p._onAuthFailure = p._gazeOriginalOnAuthFailure;
          if (p._gazeOriginalOnAuthCancel)
            p._onAuthCancel = p._gazeOriginalOnAuthCancel;
          if (p._gazeOriginalOnUnlock) p._onUnlock = p._gazeOriginalOnUnlock;
          if (p._gazeOriginalOnKeyPress) p._onKeyPress = p._gazeOriginalOnKeyPress;
          delete p._gazeOverridden;
          delete p._gazeOriginalOnAuthInfo;
          delete p._gazeOriginalOnAuthPrompt;
          delete p._gazeOriginalOnAuthError;
          delete p._gazeOriginalOnAuthSuccess;
          delete p._gazeOriginalOnAuthFailure;
          delete p._gazeOriginalOnAuthCancel;
          delete p._gazeOriginalOnUnlock;
          delete p._gazeOriginalOnKeyPress;
        }
      } catch (e) {}
      this._unlockPatched = false;
    }
  }
}

let extensionInstance = null;

function init(metadata) {
  extensionInstance = new GazeCinnamonExtension(metadata);
}

function enable() {
  extensionInstance?.enable();
}

function disable() {
  extensionInstance?.disable();
}
