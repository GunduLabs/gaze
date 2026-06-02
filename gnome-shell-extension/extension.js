import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension, InjectionManager} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Util from 'resource:///org/gnome/shell/gdm/util.js';
import * as PolkitAgent from 'resource:///org/gnome/shell/ui/components/polkitAgent.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

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
  </interface>
</node>
`;
const GazeProxy = Gio.DBusProxy.makeProxyWrapper(GAZE_DBUS_INTERFACE);

const FACE_SERVICE_NAME = 'gdm-face';
const EXTENSION_SCHEMA_ID = 'org.gnome.shell.extensions.gaze';
const FACE_AUTHENTICATION_KEY = 'enable-face-authentication';
const MAX_TRIES_KEY = 'max-face-tries';
const FACE_ERROR_TIMEOUT_WAIT = 15;

const GENERIC_ERROR_MAP = new Map([
    ['Sorry, that did not work. Please try again.',
        'Sorry, face authentication did not work. Please try again.'],
    ['Sorry, that didn\u2019t work. Please try again.',
        'Sorry, face authentication did not work. Please try again.'],
    ['You reached the maximum authentication attempts, please try another method',
        'You reached the maximum face authentication attempts, please try another method'],
]);

const FACE_STATUS_UPDATES = new Set([
    'No faces detected. Please look at the camera...',
    'Face is clipped. Please move fully into frame...',
    'Please center your face...',
]);

function clearFaceFailureTimeout(verifier) {
    if (verifier._gazeFaceFailedId) {
        GLib.source_remove(verifier._gazeFaceFailedId);
        verifier._gazeFaceFailedId = 0;
    }
}

export default class GazeFaceAuthExtension extends Extension {
    enable() {
        this._injectionManager = new InjectionManager();
        this._extensionSettings = new Gio.Settings({schema_id: EXTENSION_SCHEMA_ID});

        // Initialize D-Bus proxy and register extension
        try {
            this._dbusProxy = new GazeProxy(
                Gio.DBus.system,
                'com.gundulabs.Gaze',
                '/com/gundulabs/Gaze',
                (proxy, error) => {
                    if (error) {
                        return;
                    }
                    try {
                        proxy.RegisterExtensionRemote(true);
                    } catch (e) {
                    }
                }
            );

            this._dbusProxy.connect('notify::g-name-owner', () => {
                if (this._dbusProxy.g_name_owner) {
                    try {
                        this._dbusProxy.RegisterExtensionRemote(true);
                    } catch (e) {
                    }
                }
            });
        } catch (e) {
        }

        const proto = Util.ShellUserVerifier.prototype;
        const extensionSettings = this._extensionSettings;

        const getFaceEnabled = () => extensionSettings.get_boolean(FACE_AUTHENTICATION_KEY);
        const getMaxTries = () => Math.max(1, extensionSettings.get_int(MAX_TRIES_KEY));

        const dbusProxy = this._dbusProxy;

        // UI Interceptor Hook for Polkit Confirmation
        this._injectionManager.overrideMethod(PolkitAgent.Component.prototype, '_onInitiate',
            original => {
                return function (cookie, identity, actionId, message, iconName, details) {
                    if (dbusProxy) {
                        try {
                            dbusProxy.RegisterExtensionRemote(true);
                        } catch (e) {
                        }
                    }
                    original.call(this, cookie, identity, actionId, message, iconName, details);

                    const dialog = this._currentDialog;
                    if (!dialog) {
                        return;
                    }

                    const klass = dialog.constructor;

                    // 1. Direct session hook for the first dialog to avoid GObject bind races
                    if (dialog._session) {
                        dialog._session.connect('show-info', (session, text) => {
                            if (text && text.trim() === 'GAZE_CONFIRMATION_REQUEST') {
                                if (dialog._passwordEntry)
                                    dialog._passwordEntry.hide();

                                if (dialog._infoMessageLabel) {
                                    dialog._infoMessageLabel.text = 'Face Verified. Press Authenticate to confirm.';
                                    dialog._infoMessageLabel.show();
                                }

                                if (dialog._okButton) {
                                    dialog._okButton.reactive = true;
                                    dialog._okButton.track_hover = true;
                                }

                                dialog._confirmMode = true;
                                dialog._ensureOpen();
                            }
                        });
                    }

                    // 2. Direct _onEntryActivate override for the first dialog
                    const originalOnEntryActivate = dialog._onEntryActivate;
                    dialog._onEntryActivate = function () {
                        if (this._confirmMode) {
                            this._session.response('CONFIRM');
                        } else {
                            originalOnEntryActivate.call(this);
                        }
                    };

                    // 3. Class Prototype override for all subsequent dialogs
                    if (klass && !klass._gazeOverridden) {
                        klass._gazeOverridden = true;

                        // Override _onSessionShowInfo on prototype
                        const originalShowInfo = klass.prototype._onSessionShowInfo;
                        klass.prototype._onSessionShowInfo = function (session, text) {
                            if (text && text.trim() === 'GAZE_CONFIRMATION_REQUEST') {
                                if (this._passwordEntry) {
                                    this._passwordEntry.hide();
                                }

                                if (this._infoMessageLabel) {
                                    this._infoMessageLabel.text = 'Face Verified. Press Authenticate to confirm.';
                                    this._infoMessageLabel.show();
                                }

                                if (this._okButton) {
                                    this._okButton.reactive = true;
                                    this._okButton.track_hover = true;
                                }

                                this._confirmMode = true;
                                this._ensureOpen();
                            } else {
                                originalShowInfo.call(this, session, text);
                            }
                        };

                        // Override _onEntryActivate on prototype
                        const originalProtoOnEntryActivate = klass.prototype._onEntryActivate;
                        klass.prototype._onEntryActivate = function () {
                            if (this._confirmMode) {
                                this._session.response('CONFIRM');
                            } else {
                                originalProtoOnEntryActivate.call(this);
                            }
                        };
                    }
                };
            });

        // Restart polkitAgent component to bind overrides
        const manager = Main.componentManager;
        if (manager) {
            manager._disableComponent('polkitAgent');
            delete manager._allComponents['polkitAgent'];
            manager._enableComponent('polkitAgent').catch(e => {
            });
        }

        this._injectionManager.overrideMethod(proto, '_updateEnabledServices',
            original => {
                return function () {
                    original.call(this);
                    this._faceEnabled = getFaceEnabled();
                    this._faceMaxTries = getMaxTries();
                };
            });

        this._injectionManager.overrideMethod(proto, '_beginVerification',
            original => {
                return function () {
                    original.call(this);

                    this._faceEnabled = getFaceEnabled();
                    this._faceMaxTries = getMaxTries();

                    if (this._userName && this._faceEnabled && !this.serviceIsForeground(FACE_SERVICE_NAME))
                        this._startService(FACE_SERVICE_NAME);
                };
            });

        proto.serviceIsFace = function (serviceName) {
            return this._faceEnabled && serviceName === FACE_SERVICE_NAME;
        };

        proto.serviceIsBiometric = function (serviceName) {
            return (this.serviceIsFace(serviceName) || this.serviceIsFingerprint(serviceName)) &&
                !this.serviceIsForeground(serviceName);
        };

        proto._canFaceRetry = function () {
            return this._userName &&
                (this._reauthOnly || this._failCounter < (this._faceMaxTries ?? 1));
        };

        proto._getHint = function () {
            const faceActive = this._activeServices.has(FACE_SERVICE_NAME);
            const fpActive = this._activeServices.has(Util.FINGERPRINT_SERVICE_NAME);

            if (faceActive && fpActive) {
                return this._fingerprintReaderType === 2
                    ? '(or look at the camera or swipe finger)'
                    : '(or look at the camera or place finger on reader)';
            }

            if (faceActive)
                return '(or look at the camera)';

            if (fpActive) {
                return this._fingerprintReaderType === 2
                    ? '(or swipe finger across reader)'
                    : '(or place finger on reader)';
            }

            return null;
        };

        this._injectionManager.overrideMethod(proto, '_onConversationStarted',
            original => {
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
            });

        this._injectionManager.overrideMethod(proto, '_onInfo',
            original => {
                return function (client, serviceName, info) {
                    if (this.serviceIsFace(serviceName)) {
                        const text = info?.trim();
                        if (!text || !FACE_STATUS_UPDATES.has(text))
                            return;

                        this._filterServiceMessages(serviceName, Util.MessageType.HINT);
                        this._queueMessage(serviceName, text, Util.MessageType.HINT);
                        return;
                    }

                    if (this.serviceIsBiometric(serviceName))
                        return;

                    original.call(this, client, serviceName, info);
                };
            });

        this._injectionManager.overrideMethod(proto, '_onProblem',
            original => {
                return function (client, serviceName, problem) {
                    if (this.serviceIsFace(serviceName)) {
                        const mapped = GENERIC_ERROR_MAP.get(problem) ?? problem;
                        this._queuePriorityMessage(serviceName, mapped, Util.MessageType.ERROR);

                        this._failCounter++;

                        if (!this._canFaceRetry()) {
                            clearFaceFailureTimeout(this);

                            const cancellable = this._cancellable;
                            this._gazeFaceFailedId = GLib.timeout_add_once(GLib.PRIORITY_DEFAULT,
                                FACE_ERROR_TIMEOUT_WAIT, () => {
                                    this._gazeFaceFailedId = 0;
                                    if (cancellable && !cancellable.is_cancelled()) {
                                        this._verificationFailed(serviceName, false)
                                            .catch(error => logError(error, '[gaze] Failed to stop face auth after max tries'));
                                    }
                                });
                        }

                        return;
                    }

                    original.call(this, client, serviceName, problem);
                };
            });

        this._injectionManager.overrideMethod(proto, '_onConversationStopped',
            original => {
                return function (client, serviceName) {
                    original.call(this, client, serviceName);

                    if (this.serviceIsBiometric(serviceName)) {
                        const hint = this._getHint();
                        if (hint) {
                            const bgSvc = [...this._activeServices].find(s =>
                                this.serviceIsBiometric(s)
                            );

                            if (bgSvc) {
                                this._filterServiceMessages(bgSvc, Util.MessageType.HINT);
                                this._queueMessage(bgSvc, hint, Util.MessageType.HINT);
                            }
                        }
                    }
                };
            });

        this._injectionManager.overrideMethod(proto, '_onReset',
            original => {
                return function () {
                    clearFaceFailureTimeout(this);
                    original.call(this);
                };
            });

        this._injectionManager.overrideMethod(proto, '_verificationFailed',
            original => {
                return async function (serviceName, shouldRetry) {
                    if (serviceName === FACE_SERVICE_NAME)
                        clearFaceFailureTimeout(this);

                    return original.call(this, serviceName, shouldRetry);
                };
            });
    }

    disable() {
        if (this._dbusProxy) {
            try {
                this._dbusProxy.RegisterExtensionRemote(false);
            } catch (e) {
                logError(e, '[gaze] Failed to unregister extension');
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

        // Restart polkitAgent component to restore standard class
        const manager = Main.componentManager;
        if (manager) {
            manager._disableComponent('polkitAgent');
            delete manager._allComponents['polkitAgent'];
            manager._enableComponent('polkitAgent').catch(e => {
            });
        }
    }
}
