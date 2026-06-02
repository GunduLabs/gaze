import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

const EXTENSION_SCHEMA_ID = 'org.gnome.shell.extensions.gaze';
const GAZE_BUS_NAME = 'com.gundulabs.Gaze';
const GAZE_OBJECT_PATH = '/com/gundulabs/Gaze';

const MAX_TRIES_KEY = 'max-face-tries';
const FACE_AUTH_KEY = 'enable-face-authentication';

const GAZE_DBUS_XML = `
<node>
  <interface name="com.gundulabs.Gaze">
    <method name="Claim">
      <arg type="s" name="username" direction="in"/>
    </method>
    <method name="Release"/>
    <method name="EnrollStart">
      <arg type="s" name="face_name" direction="in"/>
    </method>
    <method name="EnrollStop"/>
    <method name="ListFaces">
      <arg type="s" name="username" direction="in"/>
      <arg type="a(su)" name="faces" direction="out"/>
    </method>
    <signal name="FaceStatus">
      <arg type="s" name="status"/>
    </signal>
    <signal name="EnrollStatus">
      <arg type="s" name="face_name"/>
      <arg type="u" name="progress"/>
      <arg type="u" name="max"/>
      <arg type="b" name="is_done"/>
      <arg type="s" name="msg"/>
      <arg type="d" name="time_remaining"/>
    </signal>
  </interface>
</node>`;

const GazeProxy = Gio.DBusProxy.makeProxyWrapper(GAZE_DBUS_XML);

const CAPTURE_STATUS_LABELS = new Map([
    ['no-face', 'Please look at the camera...'],
    ['NoFace', 'Please look at the camera...'],
    ['too-dark', 'Need more light...'],
    ['TooDark', 'Need more light...'],
    ['clipped', 'Face is clipped. Please move back...'],
    ['Clipped', 'Face is clipped. Please move back...'],
    ['not-centered', 'Please center your face...'],
    ['NotCentered', 'Please center your face...'],
    ['too-far', 'Please come closer...'],
    ['TooFar', 'Please come closer...'],
    ['too-close', 'Please back up...'],
    ['TooClose', 'Please back up...'],
    ['ready', 'Hold still...'],
    ['Ready', 'Hold still...'],
]);

const ENROLL_PROMPT_LABELS = new Map([
    ['look-straight', 'Face the camera'],
    ['LookStraight', 'Face the camera'],
    ['look-up', 'Tilt your face slightly up'],
    ['LookUp', 'Tilt your face slightly up'],
    ['look-down', 'Tilt your face slightly down'],
    ['LookDown', 'Tilt your face slightly down'],
    ['look-left', 'Turn your face slightly left'],
    ['LookLeft', 'Turn your face slightly left'],
    ['look-right', 'Turn your face slightly right'],
    ['LookRight', 'Turn your face slightly right'],
    ['db-failed', 'Database error during enrollment'],
    ['DbFailed', 'Database error during enrollment'],
    ['cancelled', 'Enrollment cancelled'],
    ['Cancelled', 'Enrollment cancelled'],
    ['captured', 'Captured'],
    ['Captured', 'Captured'],
    ['completed', 'Completed'],
    ['Completed', 'Completed'],
]);

const COMPLETED_PROMPTS = new Set(['completed', 'Completed']);

function callProxy(proxy, method, ...args) {
    return new Promise((resolve, reject) => {
        proxy[`${method}Remote`](...args, (result, error) => {
            if (error)
                reject(error);
            else
                resolve(result ?? []);
        });
    });
}

function formatError(error) {
    const message = error?.message ?? String(error);
    return message.replace(/^GDBus\.Error:[^:]+:\s*/, '').trim();
}

function labelFor(labels, value) {
    return labels.get(value) ?? value;
}
const GAZE_IFACE = 'com.gundulabs.Gaze';

function callGaze(method, params) {
    const conn = Gio.DBus.system;
    return new Promise((resolve, reject) => {
        conn.call(
            GAZE_BUS_NAME, GAZE_OBJECT_PATH, GAZE_IFACE, method, params,
            null, Gio.DBusCallFlags.ALLOW_INTERACTIVE_AUTHORIZATION, -1, null,
            (_src, res) => {
                try {
                    resolve(conn.call_finish(res));
                } catch (e) {
                    reject(e);
                }
            }
        );
    });
}
export default class GazePreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const extensionSettings = new Gio.Settings({schema_id: EXTENSION_SCHEMA_ID});

        const behaviorPage = new Adw.PreferencesPage({
            title: 'Behavior',
            icon_name: 'preferences-system-symbolic',
        });

        const behaviorGroup = new Adw.PreferencesGroup({
            title: 'Face authentication',
            description: 'Settings are stored in your current dconf profile.',
        });

        const faceRow = new Adw.SwitchRow({
            title: 'Enable face authentication',
            active: extensionSettings.get_boolean(FACE_AUTH_KEY),
        });

        faceRow.connect('notify::active', row => {
            extensionSettings.set_boolean(FACE_AUTH_KEY, row.active);
        });
        extensionSettings.connect(`changed::${FACE_AUTH_KEY}`, () => {
            faceRow.set_active(extensionSettings.get_boolean(FACE_AUTH_KEY));
        });
        behaviorGroup.add(faceRow);

        const triesRow = new Adw.SpinRow({
            title: 'Maximum face tries',
            adjustment: new Gtk.Adjustment({
                lower: 1,
                upper: 20,
                step_increment: 1,
                page_increment: 1,
                value: extensionSettings.get_int(MAX_TRIES_KEY),
            }),
        });
        extensionSettings.bind(
            MAX_TRIES_KEY,
            triesRow,
            'value',
            Gio.SettingsBindFlags.DEFAULT
        );
        behaviorGroup.add(triesRow);

        const username = GLib.get_user_name();
        let proxyPromise = null;
        let profileProxy = null;
        let enrollmentActive = false;
        let claimActive = false;
        let activeFaceName = null;

        const profileGroup = new Adw.PreferencesGroup({
            title: 'Face profiles',
            description: 'Create or refine a face profile without opening a terminal.',
        });

        const userRow = new Adw.ActionRow({
            title: 'User',
            subtitle: username,
        });
        profileGroup.add(userRow);

        const profileNameRow = new Adw.EntryRow({
            title: 'Profile name',
            text: 'default',
        });
        profileGroup.add(profileNameRow);

        const profilesRow = new Adw.ActionRow({
            title: 'Enrolled profiles',
            subtitle: 'Loading...',
        });
        const refreshButton = new Gtk.Button({
            label: 'Refresh',
            valign: Gtk.Align.CENTER,
        });
        profilesRow.add_suffix(refreshButton);
        profilesRow.activatable_widget = refreshButton;
        profileGroup.add(profilesRow);

        const progressRow = new Adw.ActionRow({
            title: 'Enrollment progress',
            subtitle: 'Not started',
        });
        const progressBar = new Gtk.ProgressBar({
            width_request: 160,
            valign: Gtk.Align.CENTER,
        });
        progressRow.add_suffix(progressBar);
        profileGroup.add(progressRow);

        const cameraRow = new Adw.ActionRow({
            title: 'Camera status',
            subtitle: 'Idle',
        });
        profileGroup.add(cameraRow);

        const enrollRow = new Adw.ActionRow({
            title: 'Create or refine profile',
            subtitle: 'Follow the prompts and keep your face in frame.',
        });
        const buttonBox = new Gtk.Box({
            orientation: Gtk.Orientation.HORIZONTAL,
            spacing: 6,
            valign: Gtk.Align.CENTER,
        });
        const cancelButton = new Gtk.Button({
            label: 'Cancel',
            sensitive: false,
            valign: Gtk.Align.CENTER,
        });
        const enrollButton = new Gtk.Button({
            label: 'Start',
            valign: Gtk.Align.CENTER,
        });
        enrollButton.add_css_class('suggested-action');
        buttonBox.append(cancelButton);
        buttonBox.append(enrollButton);
        enrollRow.add_suffix(buttonBox);
        enrollRow.activatable_widget = enrollButton;
        profileGroup.add(enrollRow);

        function setEnrollmentActive(active) {
            enrollmentActive = active;
            enrollButton.sensitive = !active;
            cancelButton.sensitive = active;
            refreshButton.sensitive = !active;
            profileNameRow.sensitive = !active;
        }

        function setError(error) {
            progressRow.subtitle = formatError(error);
            cameraRow.subtitle = 'Idle';
        }

        function releaseClaim() {
            if (!claimActive || !profileProxy)
                return Promise.resolve();

            claimActive = false;
            return callProxy(profileProxy, 'Release').catch(() => {});
        }

        function ensureProxy() {
            if (proxyPromise)
                return proxyPromise;

            proxyPromise = new Promise((resolve, reject) => {
                const proxy = new GazeProxy(
                    Gio.DBus.system,
                    GAZE_BUS_NAME,
                    GAZE_OBJECT_PATH,
                    (createdProxy, error) => {
                        if (error) {
                            proxyPromise = null;
                            reject(error);
                            return;
                        }

                        profileProxy = createdProxy;
                        profileProxy.connectSignal('FaceStatus', (_proxy, _sender, [status]) => {
                            if (!enrollmentActive)
                                return;

                            cameraRow.subtitle = labelFor(CAPTURE_STATUS_LABELS, status);
                        });
                        profileProxy.connectSignal('EnrollStatus', (_proxy, _sender, args) => {
                            const [faceName, progress, max, isDone, msg, timeRemaining] = args;
                            if (!enrollmentActive || faceName !== activeFaceName)
                                return;

                            const prompt = labelFor(ENROLL_PROMPT_LABELS, msg);
                            const remaining = timeRemaining > 0
                                ? `, about ${Math.ceil(timeRemaining)}s left`
                                : '';
                            progressRow.subtitle = `${prompt} (${progress}/${max}${remaining})`;
                            progressBar.fraction = max > 0 ? progress / max : 0;

                            if (!isDone)
                                return;

                            const completed = COMPLETED_PROMPTS.has(msg);
                            void releaseClaim().then(() => {
                                setEnrollmentActive(false);
                                activeFaceName = null;
                                cameraRow.subtitle = 'Idle';
                                progressRow.subtitle = completed
                                    ? `Profile "${faceName}" saved`
                                    : prompt;
                                progressBar.fraction = completed ? 1 : progressBar.fraction;
                                return refreshProfiles();
                            });
                        });
                        resolve(profileProxy);
                    }
                );
                void proxy;
            });

            return proxyPromise;
        }

        async function refreshProfiles() {
            try {
                const proxy = await ensureProxy();
                const [faces = []] = await callProxy(proxy, 'ListFaces', username);
                if (!faces.length) {
                    profilesRow.subtitle = 'No profiles enrolled yet';
                    return;
                }

                profilesRow.subtitle = faces
                    .map(([name, count]) => `${name} (${count})`)
                    .join(', ');
            } catch (error) {
                profilesRow.subtitle = `Unavailable: ${formatError(error)}`;
            }
        }

        async function startEnrollment() {
            if (enrollmentActive)
                return;

            const faceName = profileNameRow.text.trim();
            if (!faceName) {
                progressRow.subtitle = 'Enter a profile name before starting enrollment';
                return;
            }

            setEnrollmentActive(true);
            activeFaceName = faceName;
            progressBar.fraction = 0;
            progressRow.subtitle = 'Claiming camera...';
            cameraRow.subtitle = 'Starting...';

            try {
                const proxy = await ensureProxy();
                await callProxy(proxy, 'Claim', username);
                claimActive = true;
                await callProxy(proxy, 'EnrollStart', faceName);
                progressRow.subtitle = 'Waiting for capture prompt...';
            } catch (error) {
                await releaseClaim();
                setEnrollmentActive(false);
                activeFaceName = null;
                setError(error);
            }
        }

        async function cancelEnrollment() {
            if (!enrollmentActive)
                return;

            try {
                if (profileProxy)
                    await callProxy(profileProxy, 'EnrollStop');
            } catch (error) {
                setError(error);
            } finally {
                await releaseClaim();
                setEnrollmentActive(false);
                activeFaceName = null;
                progressRow.subtitle = 'Enrollment cancelled';
                cameraRow.subtitle = 'Idle';
                progressBar.fraction = 0;
                await refreshProfiles();
            }
        }

        refreshButton.connect('clicked', () => {
            void refreshProfiles();
        });
        enrollButton.connect('clicked', () => {
            void startEnrollment();
        });
        cancelButton.connect('clicked', () => {
            void cancelEnrollment();
        });
        window.connect('close-request', () => {
            if (enrollmentActive)
                void cancelEnrollment();
            return false;
        });

        void refreshProfiles();

        behaviorPage.add(profileGroup);
        behaviorPage.add(behaviorGroup);

        const loginGroup = new Adw.PreferencesGroup({
            title: 'GDM login screen',
            description:
                'Enable face authentication at the GDM login screen. ' +
                'Requires administrator authorization. ' +
                'Note: GNOME keyring is normally unlocked by your password, ' +
                'so logging in with face only may leave it locked.',
        });

        const gdmRow = new Adw.SwitchRow({
            title: 'Enable face auth at GDM login',
            active: false,
            sensitive: false,
        });

        let suppressGdmNotify = false;
        const setGdmRow = active => {
            suppressGdmNotify = true;
            gdmRow.set_active(active);
            suppressGdmNotify = false;
        };

        callGaze('GetGdmFaceAuth', null)
            .then(result => {
                const [enabled] = result.deepUnpack();
                setGdmRow(enabled);
                gdmRow.set_sensitive(true);
            })
            .catch(error => {
                logError(error, '[gaze] Failed to read GDM face auth state');
                gdmRow.set_subtitle('Gaze daemon unavailable.');
            });

        gdmRow.connect('notify::active', row => {
            if (suppressGdmNotify)
                return;
            const desired = row.active;
            row.set_sensitive(false);
            callGaze('SetGdmFaceAuth', new GLib.Variant('(b)', [desired]))
                .then(() => {
                    gdmRow.set_sensitive(true);
                })
                .catch(error => {
                    logError(error, '[gaze] Failed to update GDM face auth');
                    setGdmRow(!desired);
                    gdmRow.set_sensitive(true);
                });
        });
        loginGroup.add(gdmRow);

        behaviorPage.add(loginGroup);

        window.add(behaviorPage);
    }
}
