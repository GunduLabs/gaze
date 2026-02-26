import Gio from 'gi://Gio';
import { Extension, InjectionManager } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Util from 'resource:///org/gnome/shell/gdm/util.js';

const FACE_SERVICE_NAME = 'gdm-face';
const GAZE_SCHEMA_ID = 'org.gnome.login-screen.gaze';
const FACE_AUTHENTICATION_KEY = 'enable-face-authentication';

const GENERIC_ERROR_MAP = new Map([
    ['Sorry, that didn\u2019t work. Please try again.',
        'Sorry, face authentication didn\u2019t work. Please try again.'],
    ['You reached the maximum authentication attempts, please try another method',
        'You reached the maximum face authentication attempts, please try another method'],
]);

export default class GazeFaceAuthExtension extends Extension {
    enable() {
        this._injectionManager = new InjectionManager();
        this._gazeSettings = new Gio.Settings({ schema_id: GAZE_SCHEMA_ID });
        const proto = Util.ShellUserVerifier.prototype;
        const gazeSettings = this._gazeSettings;

        this._injectionManager.overrideMethod(proto, '_updateEnabledServices',
            original => {
                return function () {
                    original.call(this);
                    this._faceEnabled = gazeSettings.get_boolean(FACE_AUTHENTICATION_KEY);
                };
            });

        this._injectionManager.overrideMethod(proto, '_beginVerification',
            original => {
                return function () {
                    original.call(this);
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

        proto._getHint = function () {
            const faceActive = this._activeServices.has(FACE_SERVICE_NAME);
            const fpActive = this._activeServices.has(Util.FINGERPRINT_SERVICE_NAME);

            if (faceActive && fpActive) {
                return this._fingerprintReaderType === 2 // SWIPE
                    ? '(or look at the camera or swipe finger)'
                    : '(or look at the camera or place finger on reader)';
            }

            if (faceActive)
                return '(or look at the camera)';

            if (fpActive) {
                return this._fingerprintReaderType === 2 // SWIPE
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
                    if (this.serviceIsBiometric(serviceName)) {
                        return;
                    }

                    original.call(this, client, serviceName, info);
                };
            });

        this._injectionManager.overrideMethod(proto, '_onProblem',
            original => {
                return function (client, serviceName, problem) {
                    if (this.serviceIsFace(serviceName)) {
                        const mapped = GENERIC_ERROR_MAP.get(problem) ?? problem;
                        this._queuePriorityMessage(serviceName, mapped, Util.MessageType.ERROR);
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
    }

    disable() {
        const proto = Util.ShellUserVerifier.prototype;
        delete proto.serviceIsFace;
        delete proto.serviceIsBiometric;
        delete proto._getHint;

        this._injectionManager.clear();
        this._injectionManager = null;
        this._gazeSettings = null;
    }
}
