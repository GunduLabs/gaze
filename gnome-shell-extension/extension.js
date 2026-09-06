// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import Gio from "gi://Gio";
import GLib from "gi://GLib";
import GObject from "gi://GObject";
import St from "gi://St";
import Clutter from "gi://Clutter";
import {
  Extension,
  InjectionManager,
  gettext as _,
} from "resource:///org/gnome/shell/extensions/extension.js";
import * as Animation from "resource:///org/gnome/shell/ui/animation.js";
import * as Batch from "resource:///org/gnome/shell/gdm/batch.js";
import * as Util from "resource:///org/gnome/shell/gdm/util.js";
import * as AuthPrompt from "resource:///org/gnome/shell/gdm/authPrompt.js";
import * as PolkitAgent from "resource:///org/gnome/shell/ui/components/polkitAgent.js";
import * as Main from "resource:///org/gnome/shell/ui/main.js";

const GAZE_DBUS_INTERFACE = `
<node>
  <interface name="com.gundulabs.Gaze">
    <property name="PamInternal" type="as" access="readwrite"/>
    <method name="AddPamInternal">
      <arg name="service" type="s" direction="in"/>
    </method>
    <method name="RemovePamInternal">
      <arg name="service" type="s" direction="in"/>
    </method>
    <method name="ClearPamInternal">
    </method>
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
// GNOME 50 moved these out of gdm/util.js without leaving a re-export behind.
// The values are part of the GDM wire protocol, so hardcoding is safe.
const MESSAGE_TYPE = Util.MessageType ?? {
  NONE: 0,
  HINT: 1,
  INFO: 2,
  ERROR: 3,
};
const FINGERPRINT_SERVICE_NAME =
  Util.FINGERPRINT_SERVICE_NAME ?? "gdm-fingerprint";
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

const FACE_HINT_TEXT = "(or look at the camera)";
const LEGACY_AUTH_SERVICES_ROLE = "fingerprint";

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

const INTERNAL_ERROR_MAP = new Map([
  [
    GAZE_MSG_FACE_NOT_RECOGNIZED,
    _("Face not recognized. Please enter your password.") ||
      "Face not recognized. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_NOT_DETECTED,
    _("Face not detected. Please enter your password.") ||
      "Face not detected. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_TOO_DARK,
    _("Too dark for face authentication. Please enter your password.") ||
      "Too dark for face authentication. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_TIMED_OUT,
    _("Face authentication timed out. Please enter your password.") ||
      "Face authentication timed out. Please enter your password.",
  ],
  [
    GAZE_MSG_FACE_UNAVAILABLE,
    _("Face authentication unavailable. Please enter your password.") ||
      "Face authentication unavailable. Please enter your password.",
  ],
]);

// RHEL 10's backport declares the AuthServices signals with positional
// parameters; GNOME 50 upstream collapsed them into a single params object.
const authServicesShapes = new WeakMap();

const usesParamsObject = (services) => {
  const proto = Object.getPrototypeOf(services);
  const cached = authServicesShapes.get(proto);
  if (cached !== undefined) return cached;

  const structural = typeof services._handleGetSupportedRoles === "function";
  let paramsObject;
  try {
    const id = GObject.signal_lookup(
      "queue-message",
      services.constructor.$gtype,
    );
    paramsObject = id ? GObject.signal_query(id).n_params === 1 : structural;
  } catch (e) {
    paramsObject = structural;
  }

  authServicesShapes.set(proto, paramsObject);
  return paramsObject;
};

const emitFilterMessages = (services, serviceName, messageType) => {
  if (usesParamsObject(services))
    services.emit("filter-messages", { serviceName, messageType });
  else services.emit("filter-messages", serviceName, messageType);
};

const emitQueueMessage = (services, serviceName, message, messageType) => {
  if (usesParamsObject(services))
    services.emit("queue-message", { serviceName, message, messageType });
  else services.emit("queue-message", serviceName, message, messageType);
};

const emitQueuePriorityMessage = (services, serviceName, message, messageType) => {
  if (usesParamsObject(services))
    services.emit("queue-priority-message", {
      serviceName,
      message,
      messageType,
    });
  else services.emit("queue-priority-message", serviceName, message, messageType);
};

const emitAskQuestion = (services, serviceName, question, secret) => {
  if (usesParamsObject(services)) {
    services.emit("ask-question", {
      serviceName,
      question,
      secret,
      answerHandler: (answer) => services._handleAnswerQuery(serviceName, answer),
    });
  } else {
    services.emit("ask-question", serviceName, question, secret);
  }
};

const isLegacyAuthServices = (services) => {
  if (!services || typeof services !== "object") return false;
  if (typeof services._startService !== "function") return false;

  const roles = services.constructor?.SupportedRoles;
  if (Array.isArray(roles)) return roles.includes(LEGACY_AUTH_SERVICES_ROLE);

  const roleToService = services.constructor?.RoleToService;
  if (roleToService && typeof roleToService === "object")
    return LEGACY_AUTH_SERVICES_ROLE in roleToService;

  return /Legacy/.test(services.constructor?.name ?? "");
};

// RHEL 10 keeps one named property per architecture; GNOME 50 upstream
// replaced them with a single _authServices array.
const collectAuthServices = (verifier) => {
  const collected = [];
  const add = (candidate) => {
    if (
      candidate &&
      typeof candidate === "object" &&
      !collected.includes(candidate)
    )
      collected.push(candidate);
  };

  if (Array.isArray(verifier._authServices)) verifier._authServices.forEach(add);
  else add(verifier._authServices);

  for (const key of Object.keys(verifier)) {
    if (key !== "_authServices" && key.startsWith("_authServices"))
      add(verifier[key]);
  }

  return collected;
};

// The emitter is the first argument everywhere except GNOME 50 upstream,
// which passes a lone params object instead.
const readAskQuestionArgs = (args) => {
  const [first, second, third] = args;
  if (first && typeof first === "object" && "serviceName" in first)
    return { serviceName: first.serviceName, question: first.question };
  if (typeof first === "string") return { serviceName: first, question: second };
  return { serviceName: second, question: third };
};


const CONFIRMATION_QUESTION = "Face Verified. Press Enter to confirm.";
const CONFIRMATION_DIALOG_LABEL =
  "Face verified. Press Enter or click Authenticate to confirm.";

const isConfirmationMessage = (text) => {
  const trimmed = text?.trim();
  return (
    trimmed === GAZE_REQUIRE_CONFIRMATION || trimmed === CONFIRMATION_QUESTION
  );
};

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
  dialog._session.response(GAZE_CONFIRMED);
  dialog._passwordEntry?.set_text("");
  if (dialog._passwordEntry) dialog._passwordEntry.reactive = false;
  if (dialog._okButton) dialog._okButton.reactive = false;
};

const ensureAuthPromptConfirmButton = (authPrompt, extension) => {
  if (authPrompt._confirmButton) return authPrompt._confirmButton;

  const button = new St.Button({
    style_class: "button modal-dialog-button default",
    style: "padding: 9px 24px; border-radius: 12px;",
    label: _("Confirm Face Unlock") || "Confirm Face Unlock",
    can_focus: true,
    reactive: true,
    x_align: Clutter.ActorAlign.FILL,
    x_expand: true,
    y_align: Clutter.ActorAlign.CENTER,
    visible: false,
  });

  button.connect("clicked", () => {
    if (authPrompt._confirmMode) {
      respondToAuthPromptConfirm(authPrompt);
    }
  });

  authPrompt._confirmButton = button;
  if (extension?._activeConfirmWidgets) {
    extension._activeConfirmWidgets.add(button);
  }

  const spinnerBin = new St.Bin({
    x_align: Clutter.ActorAlign.CENTER,
    y_align: Clutter.ActorAlign.CENTER,
    x_expand: true,
    visible: false,
  });
  const spinner = new Animation.Spinner(16);
  spinnerBin.set_child(spinner);
  authPrompt._confirmSpinnerBin = spinnerBin;
  authPrompt._confirmSpinner = spinner;
  if (extension?._activeConfirmWidgets) {
    extension._activeConfirmWidgets.add(spinnerBin);
  }

  if (authPrompt._mainBox) {
    if (authPrompt._defaultButtonWell) {
      authPrompt._mainBox.insert_child_below(
        button,
        authPrompt._defaultButtonWell,
      );
      authPrompt._mainBox.insert_child_below(
        spinnerBin,
        authPrompt._defaultButtonWell,
      );
    } else {
      authPrompt._mainBox.add_child(button);
      authPrompt._mainBox.add_child(spinnerBin);
    }
  }

  return button;
};

const enterAuthPromptConfirmMode = (authPrompt, serviceName, extension) => {
  try {
    authPrompt._queryingService = serviceName;
    authPrompt._confirmMode = true;

    if (authPrompt._passwordEntry) {
      authPrompt._passwordEntry.set_text("");
      authPrompt._passwordEntry.hide();
      authPrompt._passwordEntry.reactive = false;
    }
    if (authPrompt._textEntry) {
      authPrompt._textEntry.set_text("");
      authPrompt._textEntry.hide();
      authPrompt._textEntry.reactive = false;
    }
    if (authPrompt._entry) {
      authPrompt._entry.hide();
      authPrompt._entry.reactive = false;
    }
    // Only present on shells carrying the unified-auth rework.
    authPrompt._entryArea?.hide();
    if (authPrompt._capsLockWarningLabel) {
      authPrompt._capsLockWarningLabel.hide();
    }
    if (authPrompt._authList) {
      authPrompt._authList.hide();
    }

    authPrompt.setMessage(null);
    if (authPrompt._message) {
      authPrompt._message.text = "";
      authPrompt._message.opacity = 0;
      authPrompt._message.hide();
    }

    if (authPrompt._confirmSpinner) {
      authPrompt._confirmSpinner.stop();
    }
    if (authPrompt._confirmSpinnerBin) {
      authPrompt._confirmSpinnerBin.hide();
    }

    const button = ensureAuthPromptConfirmButton(authPrompt, extension);
    button.show();
    button.reactive = true;
    button.can_focus = true;
    button.grab_key_focus();

    authPrompt.emit("prompted");
  } catch (e) {
    logError(e, "[gaze] Failed to enter confirm mode");
    exitAuthPromptConfirmMode(authPrompt);
    authPrompt.setMessage(
      _("Failed to show confirmation prompt") ||
        "Failed to show confirmation prompt",
      MESSAGE_TYPE.ERROR,
    );
  }
};

const respondToAuthPromptConfirm = (authPrompt) => {
  if (!authPrompt._confirmMode) return;
  authPrompt._confirmMode = false;
  authPrompt._confirmSucceeded = false;

  if (authPrompt._confirmButton) {
    authPrompt._confirmButton.reactive = false;
    authPrompt._confirmButton.hide();
  }

  if (authPrompt._confirmSpinnerBin && authPrompt._confirmSpinner) {
    authPrompt._confirmSpinnerBin.show();
    authPrompt._confirmSpinner.play();
  } else {
    authPrompt.startSpinning();
  }

  authPrompt.verificationStatus =
    AuthPrompt.AuthPromptStatus.VERIFICATION_IN_PROGRESS;
  authPrompt.updateSensitivity(false);

  const serviceName = authPrompt._queryingService;
  if (serviceName && authPrompt._userVerifier) {
    authPrompt._userVerifier.answerQuery(serviceName, GAZE_CONFIRMED);
  }

  authPrompt.emit("next");
};

const exitAuthPromptConfirmMode = (authPrompt) => {
  if (
    !authPrompt._confirmMode &&
    !authPrompt._confirmButton?.visible &&
    !authPrompt._confirmSpinnerBin?.visible
  )
    return;
  authPrompt._confirmMode = false;

  if (authPrompt._confirmSpinner) {
    authPrompt._confirmSpinner.stop();
  }
  if (authPrompt._confirmSpinnerBin) {
    authPrompt._confirmSpinnerBin.hide();
  }
  if (authPrompt._confirmButton) {
    authPrompt._confirmButton.reactive = false;
    authPrompt._confirmButton.hide();
  }

  if (authPrompt._userVerifier) {
    authPrompt._userVerifier._faceConfirmPending = false;
    authPrompt._userVerifier._faceConfirmService = null;
  }

  if (
    authPrompt.verificationStatus ===
      AuthPrompt.AuthPromptStatus.VERIFICATION_SUCCEEDED ||
    authPrompt._confirmSucceeded
  ) {
    if (authPrompt._entry) authPrompt._entry.hide();
    if (authPrompt._passwordEntry) authPrompt._passwordEntry.hide();
    if (authPrompt._textEntry) authPrompt._textEntry.hide();
    return;
  }

  if (authPrompt._entry) {
    authPrompt._entry.show();
    authPrompt._entry.reactive = true;
  }
  authPrompt._entryArea?.show();
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
    this._activeConfirmWidgets = new Set();
    this._verifierProto = null;
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

    const registerPamInternalServices = (proxy) => {
      if (!proxy) return;
      try {
        if (typeof proxy.AddPamInternalRemote === "function") {
          proxy.AddPamInternalRemote("polkit-1");
          proxy.AddPamInternalRemote(FACE_SERVICE_NAME);
        }
      } catch (e) {
        logError(e, "[gaze] Failed to register PAM internal services");
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
            registerPamInternalServices(proxy);
          } catch (e) {}
          cacheCamera();
        },
      );

      this._dbusProxy.connect("notify::g-name-owner", () => {
        if (this._dbusProxy.g_name_owner) {
          try {
            this._dbusProxy.RegisterExtensionRemote(true);
            registerPamInternalServices(this._dbusProxy);
          } catch (e) {}
          cacheCamera();
        }
      });
    } catch (e) {}

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
              if (isConfirmationMessage(text))
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
              if (isConfirmationMessage(text)) {
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

    const installVerifierHooks = (proto) => {
      if (extension._verifierProto) return;
      extension._verifierProto = proto;

      // Shells carrying the unified-auth rework (the RHEL 10 backport of
      // gnome-shell MR !3212, and GNOME 50 upstream) moved the verification
      // lifecycle off ShellUserVerifier and onto per-architecture AuthServices
      // objects. The presence of _beginVerification tells the layouts apart.
      const legacyArch = typeof proto._beginVerification === "function";
      const injectionManager = this._injectionManager;
      const patchedAuthServices = new WeakSet();

      const startFaceOnServices = (services) => {
        if (
          !services._userVerifier ||
          services._faceAuthFailed ||
          services._activeServices?.has(FACE_SERVICE_NAME) ||
          services._unavailableServices?.has(FACE_SERVICE_NAME)
        )
          return;

        try {
          services
            ._startService(FACE_SERVICE_NAME, services._cancellable)
            ?.catch?.((e) => logError(e));
        } catch (e) {
          logError(e);
        }
      };

      const canFaceRetryOnServices = (services) => {
        const mode = services._faceRetryMode ?? "fixed";
        if (mode === "disabled") return (services._faceFailCounter ?? 0) < 1;
        if (mode === "infinite") return true;
        return (services._faceFailCounter ?? 0) < (services._faceMaxTries ?? 1);
      };

      const beginFaceOnServices = (services) => {
        if (services._faceUserName !== services._userName) {
          services._faceUserName = services._userName;
          services._faceAuthFailed = false;
          services._faceFailCounter = 0;
          services._faceStartPending = false;
          services._faceStartProbe = null;
        }

        services._faceEnabled = getFaceEnabled();
        services._faceMaxTries = getMaxTries();
        services._faceRetryMode = getRetryMode();

        const userName = services._userName;
        if (
          !userName ||
          !services._faceEnabled ||
          services._faceAuthFailed ||
          services._faceStartPending ||
          services._activeServices?.has(FACE_SERVICE_NAME)
        )
          return;

        if (
          faceCache.enrolled.get(userName) === true &&
          faceCache.camera === true
        ) {
          startFaceOnServices(services);
          return;
        }

        services._faceStartPending = true;
        const probe = {};
        services._faceStartProbe = probe;

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
              services._faceStartProbe === probe &&
              services._userName === userName
            )
              startFaceOnServices(services);
          },
          onSettled: () => {
            if (services._faceStartProbe !== probe) return;
            services._faceStartPending = false;
            services._faceStartProbe = null;
          },
          onProbeError: (error, operation) =>
            logError(
              error,
              `[gaze] Failed to ${operation}; deferring the decision to ${FACE_SERVICE_NAME} PAM`,
            ),
        });
      };

      // gdm-face is not a mechanism AuthServicesLegacy knows about, so each
      // handler below would drop its messages as belonging to an unselected
      // mechanism. We intercept them and route them ourselves.
      const patchAuthServicesProto = (authProto) => {
        if (patchedAuthServices.has(authProto)) return;
        patchedAuthServices.add(authProto);

        injectionManager.overrideMethod(
          authProto,
          "_handleBeginVerification",
          (original) => {
            return function (...args) {
              const result = original.apply(this, args);
              try {
                beginFaceOnServices(this);
              } catch (e) {
                logError(e, "[gaze] Failed to start face authentication");
              }
              return result;
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleOnInfo",
          (original) => {
            return function (serviceName, info) {
              if (serviceName === FACE_SERVICE_NAME) {
                const text = info?.trim();
                if (!text) return;

                if (
                  text === GAZE_MSG_LOOK_CAMERA ||
                  text === GAZE_MSG_LOOK_OR_PASSWORD
                ) {
                  emitFilterMessages(this, serviceName, MESSAGE_TYPE.HINT);
                  emitQueueMessage(
                    this,
                    serviceName,
                    FACE_HINT_TEXT,
                    MESSAGE_TYPE.HINT,
                  );
                  return;
                }

                if (INTERNAL_ERROR_MAP.has(text)) {
                  emitQueuePriorityMessage(
                    this,
                    serviceName,
                    INTERNAL_ERROR_MAP.get(text),
                    MESSAGE_TYPE.ERROR,
                  );
                  return;
                }

                if (!FACE_STATUS_UPDATES.has(text)) return;

                emitFilterMessages(this, serviceName, MESSAGE_TYPE.HINT);
                emitQueueMessage(this, serviceName, text, MESSAGE_TYPE.HINT);
                return;
              }

              return original.call(this, serviceName, info);
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleOnProblem",
          (original) => {
            return function (serviceName, problem) {
              if (serviceName === FACE_SERVICE_NAME) {
                const mapped =
                  INTERNAL_ERROR_MAP.get(problem) ??
                  GENERIC_ERROR_MAP.get(problem) ??
                  problem;
                emitQueuePriorityMessage(
                  this,
                  serviceName,
                  mapped,
                  MESSAGE_TYPE.ERROR,
                );
                return;
              }

              return original.call(this, serviceName, problem);
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleOnSecretInfoQuery",
          (original) => {
            return function (serviceName, secretQuestion) {
              if (isConfirmationMessage(secretQuestion)) {
                emitFilterMessages(this, serviceName, MESSAGE_TYPE.HINT);
                emitAskQuestion(
                  this,
                  serviceName,
                  GAZE_REQUIRE_CONFIRMATION,
                  true,
                );
                return;
              }

              return original.call(this, serviceName, secretQuestion);
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleOnInfoQuery",
          (original) => {
            return function (serviceName, query) {
              if (isConfirmationMessage(query)) {
                emitFilterMessages(this, serviceName, MESSAGE_TYPE.HINT);
                emitAskQuestion(
                  this,
                  serviceName,
                  GAZE_REQUIRE_CONFIRMATION,
                  false,
                );
                return;
              }

              return original.call(this, serviceName, query);
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleAnswerQuery",
          (original) => {
            return function (serviceName, answer) {
              // The original drops answers for anything but the selected
              // mechanism, swallowing the confirmation acknowledgement.
              if (serviceName === FACE_SERVICE_NAME) {
                try {
                  this._userVerifier
                    ?.call_answer_query(serviceName, answer, this._cancellable)
                    ?.catch?.((e) => logError(e));
                } catch (e) {
                  logError(e);
                }
                return;
              }

              return original.call(this, serviceName, answer);
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleOnConversationStarted",
          (original) => {
            return function (serviceName) {
              const result = original.call(this, serviceName);

              if (serviceName === FACE_SERVICE_NAME) {
                emitFilterMessages(this, serviceName, MESSAGE_TYPE.HINT);
                emitQueueMessage(
                  this,
                  serviceName,
                  FACE_HINT_TEXT,
                  MESSAGE_TYPE.HINT,
                );
              }

              return result;
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleOnConversationStopped",
          (original) => {
            return function (serviceName) {
              if (serviceName !== FACE_SERVICE_NAME)
                return original.call(this, serviceName);

              // Deliberately not chaining to the original: face runs in the
              // background, and failing it must not fail the whole conversation
              // and take the password prompt down with it.
              emitFilterMessages(this, serviceName, MESSAGE_TYPE.HINT);

              this._faceFailCounter = (this._faceFailCounter ?? 0) + 1;
              this._faceStartPending = false;
              this._faceStartProbe = null;

              if (canFaceRetryOnServices(this)) startFaceOnServices(this);
              else this._faceAuthFailed = true;

              return undefined;
            };
          },
        );

        injectionManager.overrideMethod(
          authProto,
          "_handleReset",
          (original) => {
            return function (...args) {
              this._faceUserName = null;
              this._faceFailCounter = 0;
              this._faceAuthFailed = false;
              this._faceStartPending = false;
              this._faceStartProbe = null;
              return original.apply(this, args);
            };
          },
        );
      };

      // AuthServicesLegacy is the only one claiming the fingerprint role;
      // AuthServicesSSSDSwitchable maps every role to gdm-switchable-auth.
      let warnedNoAuthServices = false;
      const patchAuthServices = (verifier) => {
        let patched = false;
        for (const services of collectAuthServices(verifier)) {
          if (!isLegacyAuthServices(services)) continue;
          patchAuthServicesProto(Object.getPrototypeOf(services));
          patched = true;
        }

        if (patched || warnedNoAuthServices) return;
        warnedNoAuthServices = true;
        console.warn(
          "[gaze] Found no auth services claiming the " +
            `${LEGACY_AUTH_SERVICES_ROLE} role; face authentication is ` +
            "unavailable on this shell",
        );
      };

      this._injectionManager.overrideMethod(proto, "begin", (original) => {
        return function (userName, hold) {
          if (this._userName !== userName) {
            this._faceAuthFailed = false;
            this._faceStartPending = false;
            this._faceStartProbe = null;
          }

          // The auth services exist from the constructor onwards, and begin()
          // runs before any handler does, so this is the earliest safe hook.
          if (!legacyArch) {
            try {
              patchAuthServices(this);
            } catch (e) {
              logError(e, "[gaze] Failed to hook the auth services");
            }
          }

          primeFaceCache(userName);
          return original.call(this, userName, hold);
        };
      });

      if (legacyArch) {
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
          const fpActive = this._activeServices.has(FINGERPRINT_SERVICE_NAME);

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
                  this._filterServiceMessages(serviceName, MESSAGE_TYPE.HINT);
                  this._queueMessage(serviceName, hint, MESSAGE_TYPE.HINT);
                }
              }
            };
          },
        );

        this._injectionManager.overrideMethod(proto, "_onInfo", (original) => {
          return function (client, serviceName, info) {
            if (this.serviceIsFace(serviceName)) {
              const text = info?.trim();
              if (!text) return;

              if (
                text === GAZE_MSG_LOOK_CAMERA ||
                text === GAZE_MSG_LOOK_OR_PASSWORD
              ) {
                this._filterServiceMessages(serviceName, MESSAGE_TYPE.HINT);
                this._queueMessage(serviceName, FACE_HINT_TEXT, MESSAGE_TYPE.HINT);
                return;
              }

              if (INTERNAL_ERROR_MAP.has(text)) {
                this._queuePriorityMessage(
                  serviceName,
                  INTERNAL_ERROR_MAP.get(text),
                  MESSAGE_TYPE.ERROR,
                );
                return;
              }

              if (FACE_STATUS_UPDATES.has(text)) {
                this._filterServiceMessages(serviceName, MESSAGE_TYPE.HINT);
                this._queueMessage(serviceName, text, MESSAGE_TYPE.HINT);
                return;
              }
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
              if (isConfirmationMessage(secretQuestion)) {
                // _filterServiceMessages only force-clears when another message is queued behind
                // the current one, so a lone hint rides out its full ~1s interval unless cleared.
                if (typeof this._clearMessageQueue === "function") {
                  this._clearMessageQueue();
                }
                this._filterServiceMessages(serviceName, MESSAGE_TYPE.HINT);
                // Enter must send confirmation, not the typed answer.
                this._faceConfirmPending = true;
                this._faceConfirmService = serviceName;
                this.emit("ask-question", serviceName, GAZE_REQUIRE_CONFIRMATION, true);
                return;
              }

              original.call(this, client, serviceName, secretQuestion);
            };
          },
        );

        this._injectionManager.overrideMethod(
          proto,
          "_onInfoQuery",
          (original) => {
            return function (client, serviceName, query) {
              if (isConfirmationMessage(query)) {
                if (typeof this._clearMessageQueue === "function") {
                  this._clearMessageQueue();
                }
                this._filterServiceMessages(serviceName, MESSAGE_TYPE.HINT);
                this._faceConfirmPending = true;
                this._faceConfirmService = serviceName;
                this.emit("ask-question", serviceName, GAZE_REQUIRE_CONFIRMATION, false);
                return;
              }

              original.call(this, client, serviceName, query);
            };
          },
        );

        this._injectionManager.overrideMethod(proto, "answerQuery", (original) => {
          return function (serviceName, answer) {
            if (this._faceConfirmPending && serviceName === this._faceConfirmService) {
              this._faceConfirmPending = false;
              this._faceConfirmService = null;
              return original.call(this, serviceName, GAZE_CONFIRMED);
            }
            return original.call(this, serviceName, answer);
          };
        });

        this._injectionManager.overrideMethod(proto, "_onProblem", (original) => {
          return function (client, serviceName, problem) {
            if (this.serviceIsFace(serviceName)) {
              const mapped =
                INTERNAL_ERROR_MAP.get(problem) ??
                GENERIC_ERROR_MAP.get(problem) ??
                problem;
              this._queuePriorityMessage(
                serviceName,
                mapped,
                MESSAGE_TYPE.ERROR,
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
                this._filterServiceMessages(serviceName, MESSAGE_TYPE.HINT);

                const hint = this._getHint();
                if (hint) {
                  const bgSvc = [...this._activeServices].find((s) =>
                    this.serviceIsBiometric(s),
                  );

                  if (bgSvc) {
                    this._filterServiceMessages(bgSvc, MESSAGE_TYPE.HINT);
                    this._queueMessage(bgSvc, hint, MESSAGE_TYPE.HINT);
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
    };

    // GNOME 50 dropped the ShellUserVerifier re-export from gdm/util.js, so
    // fall back to the instance AuthPrompt builds instead of failing to enable.
    if (Util.ShellUserVerifier?.prototype) {
      installVerifierHooks(Util.ShellUserVerifier.prototype);
    } else {
      this._injectionManager.overrideMethod(
        AuthPrompt.AuthPrompt.prototype,
        "_createUserVerifier",
        (original) => {
          return function (...args) {
            const verifier = original.apply(this, args);
            try {
              installVerifierHooks(Object.getPrototypeOf(verifier));
            } catch (e) {
              logError(e, "[gaze] Failed to hook the user verifier");
            }
            return verifier;
          };
        },
      );
    }


    const authPromptProto = AuthPrompt.AuthPrompt.prototype;

    this._injectionManager.overrideMethod(
      authPromptProto,
      "_onAskQuestion",
      (original) => {
        return function (...args) {
          const { serviceName, question } = readAskQuestionArgs(args);

          if (
            isConfirmationMessage(question) ||
            (this._userVerifier?._faceConfirmPending &&
              serviceName === this._userVerifier?._faceConfirmService)
          ) {
            enterAuthPromptConfirmMode(this, serviceName, extension);
            return;
          }
          exitAuthPromptConfirmMode(this);
          return original.apply(this, args);
        };
      },
    );

    this._injectionManager.overrideMethod(
      authPromptProto,
      "on_key_press_event",
      (original) => {
        return function (event) {
          const symbol = event.get_key_symbol();
          if (this._confirmMode) {
            if (
              symbol === Clutter.KEY_Return ||
              symbol === Clutter.KEY_KP_Enter ||
              symbol === Clutter.KEY_ISO_Enter
            ) {
              respondToAuthPromptConfirm(this);
              return Clutter.EVENT_STOP;
            }
            if (symbol === Clutter.KEY_Escape) {
              exitAuthPromptConfirmMode(this);
              this.cancel();
              return Clutter.EVENT_STOP;
            }
          }
          return original.call(this, event);
        };
      },
    );

    this._injectionManager.overrideMethod(
      authPromptProto,
      "reset",
      (original) => {
        return function (...args) {
          if (
            this.verificationStatus ===
              AuthPrompt.AuthPromptStatus.VERIFICATION_SUCCEEDED ||
            this._confirmSucceeded
          ) {
            const result = original.apply(this, args);
            if (this._entry) this._entry.hide();
            if (this._passwordEntry) this._passwordEntry.hide();
            if (this._textEntry) this._textEntry.hide();
            return result;
          }
          exitAuthPromptConfirmMode(this);
          return original.apply(this, args);
        };
      },
    );

    this._injectionManager.overrideMethod(
      authPromptProto,
      "clear",
      (original) => {
        return function (...args) {
          if (
            this.verificationStatus ===
              AuthPrompt.AuthPromptStatus.VERIFICATION_SUCCEEDED ||
            this._confirmSucceeded
          ) {
            const result = original.apply(this, args);
            if (this._entry) this._entry.hide();
            if (this._passwordEntry) this._passwordEntry.hide();
            if (this._textEntry) this._textEntry.hide();
            return result;
          }
          exitAuthPromptConfirmMode(this);
          return original.apply(this, args);
        };
      },
    );

    this._injectionManager.overrideMethod(
      authPromptProto,
      "_onVerificationFailed",
      (original) => {
        return function (...args) {
          this._confirmSucceeded = false;
          exitAuthPromptConfirmMode(this);
          return original.apply(this, args);
        };
      },
    );

    this._injectionManager.overrideMethod(
      authPromptProto,
      "updateSensitivity",
      (original) => {
        // Shells with the unified-auth rework pass {sensitive} instead of a
        // bare boolean, but still accept the boolean for compatibility.
        return function (...args) {
          const [first] = args;
          const sensitive =
            first && typeof first === "object" ? first.sensitive : first;

          if (this._confirmMode && this._confirmButton?.visible) {
            if (this._confirmButton.reactive === sensitive) return;
            this._confirmButton.reactive = sensitive;
            if (sensitive) {
              this._confirmButton.grab_key_focus();
            } else {
              this.grab_key_focus();
            }
            return;
          }
          return original.apply(this, args);
        };
      },
    );

    this._injectionManager.overrideMethod(
      authPromptProto,
      "_onVerificationComplete",
      (original) => {
        return function (...args) {
          this._confirmSucceeded = true;
          if (this._confirmSpinner) {
            this._confirmSpinner.stop();
          }
          if (this._confirmSpinnerBin) {
            this._confirmSpinnerBin.hide();
          }
          if (this._entry) {
            this._entry.hide();
          }
          if (this._passwordEntry) {
            this._passwordEntry.hide();
          }
          if (this._textEntry) {
            this._textEntry.hide();
          }
          return original.apply(this, args);
        };
      },
    );

    this._injectionManager.overrideMethod(
      authPromptProto,
      "_onDestroy",
      (original) => {
        return function (...args) {
          if (this._confirmSpinner) {
            this._confirmSpinner.stop();
            this._confirmSpinner = null;
          }
          if (this._confirmSpinnerBin) {
            if (extension?._activeConfirmWidgets) {
              extension._activeConfirmWidgets.delete(this._confirmSpinnerBin);
            }
            this._confirmSpinnerBin.destroy();
            this._confirmSpinnerBin = null;
          }
          if (this._confirmButton) {
            if (extension?._activeConfirmWidgets) {
              extension._activeConfirmWidgets.delete(this._confirmButton);
            }
            this._confirmButton.destroy();
            this._confirmButton = null;
          }
          return original.apply(this, args);
        };
      },
    );
  }

  disable() {
    this._gazeEnableToken = null;
    if (this._dbusProxy) {
      try {
        if (typeof this._dbusProxy.RemovePamInternalRemote === "function") {
          this._dbusProxy.RemovePamInternalRemote("polkit-1");
          this._dbusProxy.RemovePamInternalRemote(FACE_SERVICE_NAME);
        }
      } catch (e) {}
      try {
        this._dbusProxy.RegisterExtensionRemote(false);
      } catch (e) {
        logError(e, "[gaze] Failed to unregister extension");
      }
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

    const proto = this._verifierProto;
    if (proto) {
      delete proto.serviceIsFace;
      delete proto.serviceIsBiometric;
      delete proto._canFaceRetry;
      delete proto._getHint;
      this._verifierProto = null;
    }

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
