// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import Gio from "gi://Gio";
import GLib from "gi://GLib";
import {
  Extension,
  InjectionManager,
} from "resource:///org/gnome/shell/extensions/extension.js";
import * as Batch from "resource:///org/gnome/shell/gdm/batch.js";
import * as Util from "resource:///org/gnome/shell/gdm/util.js";
import * as PolkitAgent from "resource:///org/gnome/shell/ui/components/polkitAgent.js";
import * as Main from "resource:///org/gnome/shell/ui/main.js";

const GAZE_DBUS_INTERFACE = `
<node>
  <interface name="com.gundulabs.Gaze">
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

const FACE_SERVICE_NAME = "gdm-face";
const FACE_AUTHENTICATION_KEY = "enable-face-authentication";
const MAX_TRIES_KEY = "max-face-tries";
const RETRY_MODE_KEY = "face-retry-mode";

const replyBoolean = (result) =>
  Array.isArray(result) && typeof result[0] === "boolean" ? result[0] : null;

// These D-Bus calls only avoid starting PAM for an unenrolled user or a
// missing camera. If a probe fails, let pam_gaze make the authoritative
// decision instead of silently disabling face authentication.
const probeFaceEligibility = ({
  proxy,
  userName,
  onEnrolled,
  onCameraAvailable,
  onEligible,
  onSettled,
  onProbeError,
}) => {
  let settled = false;
  let enrollmentHandled = false;
  let cameraHandled = false;

  const settle = (eligible) => {
    if (settled) return;
    settled = true;
    try {
      if (eligible) onEligible();
    } finally {
      onSettled();
    }
  };

  const deferToPam = (error, operation) => {
    onProbeError(error, operation);
    settle(true);
  };

  if (!proxy) {
    deferToPam(
      new Error("Gaze D-Bus proxy is unavailable"),
      "check face authentication prerequisites",
    );
    return;
  }

  try {
    proxy.HasEnrolledFacesRemote(userName, (result, error) => {
      if (settled || enrollmentHandled) return;
      enrollmentHandled = true;

      if (error) {
        deferToPam(error, "check enrolled faces");
        return;
      }

      const enrolled = replyBoolean(result);
      if (enrolled === null) {
        deferToPam(
          new Error("HasEnrolledFaces returned an invalid reply"),
          "check enrolled faces",
        );
        return;
      }
      if (!enrolled) {
        settle(false);
        return;
      }
      onEnrolled();

      try {
        proxy.IsCameraAvailableRemote((cameraResult, cameraError) => {
          if (settled || cameraHandled) return;
          cameraHandled = true;

          if (cameraError) {
            deferToPam(cameraError, "check camera availability");
            return;
          }

          const cameraAvailable = replyBoolean(cameraResult);
          if (cameraAvailable === null) {
            deferToPam(
              new Error("IsCameraAvailable returned an invalid reply"),
              "check camera availability",
            );
            return;
          }
          if (cameraAvailable) {
            onCameraAvailable();
            settle(true);
          } else {
            settle(false);
          }
        });
      } catch (error) {
        deferToPam(error, "check camera availability");
      }
    });
  } catch (error) {
    deferToPam(error, "check enrolled faces");
  }
};

const recreatePolkitAgent = () => {
  const manager = Main.componentManager;
  if (!manager || Main.sessionMode.isLocked) return;

  const existing = manager._allComponents?.["polkitAgent"];
  if (existing?._currentDialog) {
    try {
      existing._currentDialog.close();
    } catch (e) {}
    existing._currentDialog = null;
  }

  manager._disableComponent("polkitAgent");
  delete manager._allComponents["polkitAgent"];
  manager._enableComponent("polkitAgent").catch(() => {});
};

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

const CONFIRMATION_REQUEST = "GAZE_CONFIRMATION_REQUEST";
const CONFIRMATION_ACK = "CONFIRM";
const CONFIRMATION_QUESTION = "Face Verified. Press Enter to confirm.";
const CONFIRMATION_DIALOG_LABEL =
  "Face verified. Press Enter or click Authenticate to confirm.";

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

const enterConfirmMode = (dialog) => {
  cancelDelayedReset(dialog);
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
  dialog._session.response(CONFIRMATION_ACK);
  dialog._passwordEntry?.set_text("");
  if (dialog._passwordEntry) dialog._passwordEntry.reactive = false;
  if (dialog._okButton) dialog._okButton.reactive = false;
};

const FACE_STATUS_UPDATES = new Set([
  "Please look at the camera...",
  "Need more light...",
  "Face is clipped. Please move back...",
  "Please center your face...",
  "Please come closer...",
  "Please back up...",
  "Hold still...",
]);

export default class GazeFaceAuthExtension extends Extension {
  enable() {
    this._injectionManager = new InjectionManager();
    this._extensionSettings = this.getSettings();
    const enableToken = {};
    this._gazeEnableToken = enableToken;

    const ext = this;
    const faceCache = { enrolled: new Map(), camera: null };

    const cacheCamera = () => {
      const proxy = ext._dbusProxy;
      if (!proxy) return;
      try {
        proxy.IsCameraAvailableRemote((result, err) => {
          if (!err && result[0] === true) faceCache.camera = true;
        });
      } catch (e) {}
    };

    const cacheEnrolled = (userName) => {
      const proxy = ext._dbusProxy;
      if (!proxy || !userName) return;
      try {
        proxy.HasEnrolledFacesRemote(userName, (result, err) => {
          if (!err && result[0] === true) faceCache.enrolled.set(userName, true);
        });
      } catch (e) {}
    };

    const primeFaceCache = (userName) => {
      cacheEnrolled(userName);
      cacheCamera();
    };

    const startFace = (verifier) => {
      if (
        !verifier._userVerifier ||
        verifier._faceAuthFailed ||
        verifier._activeServices.has(FACE_SERVICE_NAME) ||
        verifier.serviceIsForeground(FACE_SERVICE_NAME)
      )
        return;

      if (!verifier._hold?.isAcquired?.()) verifier._hold = new Batch.Hold();

      try {
        verifier._startService(FACE_SERVICE_NAME)?.catch?.((e) => logError(e));
      } catch (e) {
        logError(e);
      }
    };

    try {
      this._dbusProxy = new GazeProxy(
        Gio.DBus.system,
        "com.gundulabs.Gaze",
        "/com/gundulabs/Gaze",
        (proxy, error) => {
          if (error) {
            return;
          }
          try {
            proxy.RegisterExtensionRemote(true);
          } catch (e) {}
          cacheCamera();
        },
      );

      this._dbusProxy.connect("notify::g-name-owner", () => {
        if (this._dbusProxy.g_name_owner) {
          try {
            this._dbusProxy.RegisterExtensionRemote(true);
          } catch (e) {}
          cacheCamera();
        }
      });
    } catch (e) {}

    const proto = Util.ShellUserVerifier.prototype;
    const extensionSettings = this._extensionSettings;
    const extension = this;

    const getFaceEnabled = () =>
      extensionSettings.get_boolean(FACE_AUTHENTICATION_KEY);
    const getMaxTries = () =>
      Math.max(2, extensionSettings.get_int(MAX_TRIES_KEY));
    const getRetryMode = () => {
      try {
        return extensionSettings.get_string(RETRY_MODE_KEY);
      } catch (e) {
        return "fixed";
      }
    };

    const dbusProxy = this._dbusProxy;

    this._injectionManager.overrideMethod(
      PolkitAgent.Component.prototype,
      "_onInitiate",
      (original) => {
        return function (
          cookie,
          identity,
          actionId,
          message,
          iconName,
          details,
        ) {
          if (dbusProxy) {
            try {
              dbusProxy.RegisterExtensionRemote(true);
            } catch (e) {}
          }
          original.call(
            this,
            cookie,
            identity,
            actionId,
            message,
            iconName,
            details,
          );

          const dialog = this._currentDialog;
          if (!dialog) {
            return;
          }

          keepPasswordEntryVisible(dialog);

          const klass = dialog.constructor;

          if (dialog._session) {
            dialog._session.connect("show-info", (session, text) => {
              if (text && text.trim() === CONFIRMATION_REQUEST)
                enterConfirmMode(dialog);
            });
          }

          const originalOnEntryActivate = dialog._onEntryActivate;
          dialog._onEntryActivate = function () {
            if (this._confirmMode) {
              respondToConfirm(this);
            } else {
              originalOnEntryActivate.call(this);
            }
          };

          if (klass && !klass._gazeOverridden) {
            klass._gazeOverridden = true;
            extension._patchedDialogClass = klass;

            const originalShowInfo = klass.prototype._onSessionShowInfo;
            extension._originalDialogShowInfo = originalShowInfo;
            klass.prototype._onSessionShowInfo = function (session, text) {
              if (text && text.trim() === CONFIRMATION_REQUEST) {
                enterConfirmMode(this);
              } else {
                originalShowInfo.call(this, session, text);
              }
            };

            const originalProtoOnEntryActivate =
              klass.prototype._onEntryActivate;
            extension._originalDialogEntryActivate = originalProtoOnEntryActivate;
            klass.prototype._onEntryActivate = function () {
              if (this._confirmMode) {
                respondToConfirm(this);
              } else {
                originalProtoOnEntryActivate.call(this);
              }
            };

            const originalDestroySession = klass.prototype._destroySession;
            if (typeof originalDestroySession === "function") {
              extension._originalDialogDestroySession = originalDestroySession;
              klass.prototype._destroySession = function (delay) {
                originalDestroySession.call(this, delay);
                if (!delay) return;
                cancelDelayedReset(this);
                if (this._passwordEntry) {
                  this._passwordEntry.show();
                  this._passwordEntry.reactive = true;
                }
              };
            }
          }
        };
      },
    );

    recreatePolkitAgent();

    this._injectionManager.overrideMethod(
      proto,
      "_updateEnabledServices",
      (original) => {
        return function () {
          original.call(this);
          this._faceEnabled = getFaceEnabled();
          this._faceMaxTries = getMaxTries();
          this._faceRetryMode = getRetryMode();
        };
      },
    );

    this._injectionManager.overrideMethod(proto, "begin", (original) => {
      return function (userName, hold) {
        if (this._userName !== userName) {
          this._faceAuthFailed = false;
          this._faceStartPending = false;
          this._faceStartProbe = null;
        }
        primeFaceCache(userName);
        original.call(this, userName, hold);
      };
    });

    this._injectionManager.overrideMethod(
      proto,
      "_beginVerification",
      (original) => {
        return function () {
          original.call(this);

          this._faceEnabled = getFaceEnabled();
          this._faceMaxTries = getMaxTries();
          this._faceRetryMode = getRetryMode();
          this._faceFailCounter = 0;

          if (
            !this._userName ||
            !this._faceEnabled ||
            this._faceAuthFailed ||
            this._faceStartPending ||
            this._activeServices.has(FACE_SERVICE_NAME) ||
            this.serviceIsForeground(FACE_SERVICE_NAME)
          )
            return;

          const self = this;
          const userName = this._userName;

          if (
            faceCache.enrolled.get(userName) === true &&
            faceCache.camera === true
          ) {
            startFace(self);
            return;
          }

          this._faceStartPending = true;
          const probe = {};
          this._faceStartProbe = probe;

          probeFaceEligibility({
            proxy: dbusProxy,
            userName,
            onEnrolled: () => faceCache.enrolled.set(userName, true),
            onCameraAvailable: () => {
              faceCache.camera = true;
            },
            onEligible: () => {
              if (
                extension._gazeEnableToken === enableToken &&
                self._faceStartProbe === probe &&
                self._userName === userName
              )
                startFace(self);
            },
            onSettled: () => {
              if (self._faceStartProbe !== probe) return;
              self._faceStartPending = false;
              self._faceStartProbe = null;
            },
            onProbeError: (error, operation) =>
              logError(
                error,
                `[gaze] Failed to ${operation}; deferring the decision to ${FACE_SERVICE_NAME} PAM`,
              ),
          });
        };
      },
    );

    proto.serviceIsFace = function (serviceName) {
      return this._faceEnabled && serviceName === FACE_SERVICE_NAME;
    };

    proto.serviceIsBiometric = function (serviceName) {
      return (
        (this.serviceIsFace(serviceName) ||
          this.serviceIsFingerprint(serviceName)) &&
        !this.serviceIsForeground(serviceName)
      );
    };

    proto._canFaceRetry = function () {
      if (!this._userName) return false;
      const mode = this._faceRetryMode ?? "fixed";
      if (mode === "disabled") {
        return this._faceFailCounter < 1;
      } else if (mode === "infinite") {
        return true;
      } else {
        return this._faceFailCounter < (this._faceMaxTries ?? 1);
      }
    };

    proto._getHint = function () {
      const faceActive = this._activeServices.has(FACE_SERVICE_NAME);
      const fpActive = this._activeServices.has(Util.FINGERPRINT_SERVICE_NAME);

      if (faceActive && fpActive) {
        return this._fingerprintReaderType === 2
          ? "(or look at the camera or swipe finger)"
          : "(or look at the camera or place finger on reader)";
      }

      if (faceActive) return "(or look at the camera)";

      if (fpActive) {
        return this._fingerprintReaderType === 2
          ? "(or swipe finger across reader)"
          : "(or place finger on reader)";
      }

      return null;
    };

    this._injectionManager.overrideMethod(
      proto,
      "_onConversationStarted",
      (original) => {
        return function (client, serviceName) {
          original.call(this, client, serviceName);

          if (this.serviceIsBiometric(serviceName)) {
            const hint = this._getHint();
            if (hint) {
              this._filterServiceMessages(serviceName, Util.MessageType.HINT);
              this._queueMessage(serviceName, hint, Util.MessageType.HINT);
            }
          }
        };
      },
    );

    this._injectionManager.overrideMethod(proto, "_onInfo", (original) => {
      return function (client, serviceName, info) {
        if (this.serviceIsFace(serviceName)) {
          const text = info?.trim();
          if (!text || !FACE_STATUS_UPDATES.has(text)) return;

          this._filterServiceMessages(serviceName, Util.MessageType.HINT);
          this._queueMessage(serviceName, text, Util.MessageType.HINT);
          return;
        }

        if (this.serviceIsBiometric(serviceName)) return;

        original.call(this, client, serviceName, info);
      };
    });

    this._injectionManager.overrideMethod(
      proto,
      "_onSecretInfoQuery",
      (original) => {
        return function (client, serviceName, secretQuestion) {
          // pam_gaze defers to gdm-face on every other GDM service, but other
          // stacks can still route the token here, so match it on any service.
          if (secretQuestion?.trim() === CONFIRMATION_REQUEST) {
            // _filterServiceMessages only force-clears when another message is queued behind
            // the current one, so a lone hint rides out its full ~1s interval unless cleared.
            if (typeof this._clearMessageQueue === "function") {
              this._clearMessageQueue();
            }
            this._filterServiceMessages(serviceName, Util.MessageType.HINT);
            // Enter must send the ack token, not the empty typed answer.
            this._faceConfirmPending = true;
            this._faceConfirmService = serviceName;
            this.emit("ask-question", serviceName, CONFIRMATION_QUESTION, true);
            return;
          }

          original.call(this, client, serviceName, secretQuestion);
        };
      },
    );

    this._injectionManager.overrideMethod(proto, "answerQuery", (original) => {
      return function (serviceName, answer) {
        if (this._faceConfirmPending && serviceName === this._faceConfirmService) {
          this._faceConfirmPending = false;
          this._faceConfirmService = null;
          original.call(this, serviceName, CONFIRMATION_ACK);
          return;
        }
        original.call(this, serviceName, answer);
      };
    });

    this._injectionManager.overrideMethod(proto, "_onProblem", (original) => {
      return function (client, serviceName, problem) {
        if (this.serviceIsFace(serviceName)) {
          const mapped = GENERIC_ERROR_MAP.get(problem) ?? problem;
          this._queuePriorityMessage(
            serviceName,
            mapped,
            Util.MessageType.ERROR,
          );
          return;
        }

        original.call(this, client, serviceName, problem);
      };
    });

    this._injectionManager.overrideMethod(
      proto,
      "_onConversationStopped",
      (original) => {
        return function (client, serviceName) {
          if (serviceName === FACE_SERVICE_NAME) {
            this._faceFailCounter = (this._faceFailCounter || 0) + 1;
          }
          if (serviceName === this._faceConfirmService) {
            this._faceConfirmPending = false;
            this._faceConfirmService = null;
          }

          original.call(this, client, serviceName);

          if (this.serviceIsBiometric(serviceName)) {
            // Face has stopped, so drop its stale "look at the camera" hint;
            // otherwise it lingers once face errors out on the lock screen.
            this._filterServiceMessages(serviceName, Util.MessageType.HINT);

            const hint = this._getHint();
            if (hint) {
              const bgSvc = [...this._activeServices].find((s) =>
                this.serviceIsBiometric(s),
              );

              if (bgSvc) {
                this._filterServiceMessages(bgSvc, Util.MessageType.HINT);
                this._queueMessage(bgSvc, hint, Util.MessageType.HINT);
              }
            }
          }
        };
      },
    );

    this._injectionManager.overrideMethod(proto, "_onReset", (original) => {
      return function () {
        this._faceFailCounter = 0;
        this._faceAuthFailed = false;
        this._faceStartPending = false;
        this._faceStartProbe = null;
        this._faceConfirmPending = false;
        this._faceConfirmService = null;
        original.call(this);
      };
    });

    this._injectionManager.overrideMethod(
      proto,
      "_verificationFailed",
      (original) => {
        return async function (serviceName, shouldRetry) {
          if (serviceName === FACE_SERVICE_NAME) {
            shouldRetry = this._canFaceRetry();
            if (!shouldRetry) {
              this._faceAuthFailed = true;
            }
          }

          return original.call(this, serviceName, shouldRetry);
        };
      },
    );
  }

  disable() {
    this._gazeEnableToken = null;
    if (this._dbusProxy) {
      try {
        this._dbusProxy.RegisterExtensionRemote(false);
      } catch (e) {
        logError(e, "[gaze] Failed to unregister extension");
      }
      this._dbusProxy = null;
    }

    const proto = Util.ShellUserVerifier.prototype;
    delete proto.serviceIsFace;
    delete proto.serviceIsBiometric;
    delete proto._canFaceRetry;
    delete proto._getHint;

    this._injectionManager.clear();
    this._injectionManager = null;
    this._extensionSettings = null;

    if (this._patchedDialogClass) {
      const klass = this._patchedDialogClass;
      if (this._originalDialogShowInfo)
        klass.prototype._onSessionShowInfo = this._originalDialogShowInfo;
      if (this._originalDialogEntryActivate)
        klass.prototype._onEntryActivate = this._originalDialogEntryActivate;
      if (this._originalDialogDestroySession)
        klass.prototype._destroySession = this._originalDialogDestroySession;
      delete klass._gazeOverridden;
      this._patchedDialogClass = null;
      this._originalDialogShowInfo = null;
      this._originalDialogEntryActivate = null;
      this._originalDialogDestroySession = null;
    }

    recreatePolkitAgent();
  }
}
