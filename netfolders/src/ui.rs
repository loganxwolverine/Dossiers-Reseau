//! Interface graphique GTK4 / libadwaita : un assistant en 4 étapes.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::mounts::{self, NetMount};
use crate::privileged::{self, CredChoice, Request};
use crate::smb::{self, Credentials, Share};
use crate::bookmarks;

pub const APP_ID: &str = "io.github.netfolders.NetFolders";
const STEPS: u8 = 4;

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        Ui::build(app);
    });
    // On ne laisse pas GTK interpréter nos arguments.
    app.run_with_args::<&str>(&[])
}

/// Dossier distant en cours de sélection.
#[derive(Clone)]
struct Target {
    host: String,
    creds: Credentials,
    share: String,
    path: Vec<String>,
}

impl Target {
    fn unc(&self) -> String {
        let mut s = format!("//{}/{}", self.host, self.share);
        for p in &self.path {
            s.push('/');
            s.push_str(p);
        }
        s
    }

    fn name(&self) -> &str {
        self.path.last().unwrap_or(&self.share)
    }

    fn child(&self, dir: &str) -> Target {
        let mut t = self.clone();
        t.path.push(dir.to_string());
        t
    }
}

struct Ui {
    window: adw::ApplicationWindow,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    home_list: gtk::ListBox,
    home_empty: gtk::Label,
}

// ---------------------------------------------------------------------------
// Petits utilitaires
// ---------------------------------------------------------------------------

/// Exécute `work` dans un thread (sans figer la fenêtre) puis `done` dans le thread GTK.
fn run_bg<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(work).await.expect("le thread de travail a paniqué");
        done(r);
    });
}

fn pill_button(label: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .css_classes(["pill", "suggested-action"])
        .halign(gtk::Align::Center)
        .build()
}

/// Barre du bas contenant le bouton principal de la page.
fn bottom_bar(button: &gtk::Button) -> gtk::Box {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(18)
        .margin_start(12)
        .margin_end(12)
        .build();
    b.append(button);
    b
}

/// Passe un bouton en mode « travail en cours » (spinner + texte), ou le rétablit.
fn set_busy(button: &gtk::Button, busy: bool, label: &str) {
    if busy {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.set_halign(gtk::Align::Center);
        let spinner = gtk::Spinner::new();
        spinner.start();
        content.append(&spinner);
        content.append(&gtk::Label::new(Some(label)));
        button.set_child(Some(&content));
    } else {
        button.set_label(label);
    }
    button.set_sensitive(!busy);
}

fn page(title: &str, step: Option<u8>, content: &impl IsA<gtk::Widget>, bottom: Option<&gtk::Box>) -> adw::NavigationPage {
    let subtitle = step.map(|s| format!("Étape {s} sur {STEPS}")).unwrap_or_default();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(title, &subtitle)));
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(content));
    if let Some(b) = bottom {
        tv.add_bottom_bar(b);
    }
    adw::NavigationPage::builder().title(title).child(&tv).build()
}

fn icon(name: &str) -> gtk::Image {
    gtk::Image::from_icon_name(name)
}

fn normalize_host(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("smb://").unwrap_or(s);
    s.trim_start_matches(['/', '\\']).trim_end_matches(['/', '\\']).to_string()
}

fn home_dir() -> PathBuf {
    glib::home_dir()
}

fn valid_folder_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && name != "." && name != ".."
}

/// Vérifie que `dir` peut accueillir un dossier réseau, puis le crée.
fn prepare_local_dir(dir: &Path) -> Result<(), (&'static str, String)> {
    if mounts::is_mountpoint(dir) {
        return Err((
            "Dossier déjà utilisé",
            "Un dossier réseau est déjà monté à cet emplacement. Choisissez un autre nom.".into(),
        ));
    }
    if dir.exists() && !dir.is_dir() {
        return Err(("Nom déjà pris", "Un fichier porte déjà ce nom à cet emplacement.".into()));
    }
    if std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err((
            "Ce dossier n'est pas vide",
            "Un dossier portant ce nom existe déjà et contient des fichiers : ils seraient \
             masqués par le dossier réseau. Choisissez un autre nom."
                .into(),
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| ("Impossible de créer le dossier", e.to_string()))
}

fn missing_packages() -> Vec<String> {
    let in_path = |bin: &str| {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        dirs.extend(["/usr/sbin", "/sbin", "/usr/bin"].map(PathBuf::from));
        dirs.iter().any(|d| d.join(bin).is_file())
    };
    let mut m = Vec::new();
    if !in_path("smbclient") {
        m.push("samba-client".to_string());
    }
    if !in_path("mount.cifs") {
        m.push("cifs-utils".to_string());
    }
    m
}

// ---------------------------------------------------------------------------
// Fenêtre et pages
// ---------------------------------------------------------------------------

impl Ui {
    fn build(app: &adw::Application) {
        let nav = adw::NavigationView::new();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&nav));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("NetFolders")
            .default_width(560)
            .default_height(720)
            .width_request(360)
            .height_request(480)
            .content(&toasts)
            .build();

        let home_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let placeholder = gtk::Label::builder()
            .label("Aucun dossier réseau pour le moment.\nCliquez sur « Ajouter un dossier réseau » pour commencer.")
            .justify(gtk::Justification::Center)
            .wrap(true)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["dim-label"])
            .build();

        let ui = Rc::new(Ui { window, nav, toasts, home_list, home_empty: placeholder });
        ui.nav.add(&ui.home_page());
        ui.refresh_home();
        ui.window.present();
        ui.check_dependencies();
    }

    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    fn alert(&self, heading: &str, body: &str) {
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.add_response("ok", "Compris");
        d.present(Some(&self.window));
    }

    fn open_folder(&self, dir: &Path) {
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(dir)));
        launcher.launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
    }

    fn check_dependencies(self: &Rc<Self>) {
        let missing = missing_packages();
        if missing.is_empty() {
            return;
        }
        let d = adw::AlertDialog::new(
            Some("Composants manquants"),
            Some(&format!(
                "Pour fonctionner, l'application a besoin des paquets suivants :\n\n{}\n\n\
                 Voulez-vous les installer maintenant ? Votre mot de passe vous sera demandé.",
                missing.iter().map(|p| format!("• {p}")).collect::<Vec<_>>().join("\n")
            )),
        );
        d.add_response("quit", "Quitter");
        d.add_response("install", "Installer");
        d.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("install"));
        d.set_close_response("quit");
        let ui = self.clone();
        d.connect_response(None, move |_, resp| {
            if resp != "install" {
                ui.window.close();
                return;
            }
            ui.toast("Installation en cours…");
            let ui2 = ui.clone();
            let pkgs = missing.clone();
            run_bg(
                move || privileged::run(&Request::InstallPackages { packages: pkgs }),
                move |r| match r {
                    Ok(_) => ui2.toast("Composants installés."),
                    Err(e) => ui2.alert("Installation impossible", &e),
                },
            );
        });
        d.present(Some(&self.window));
    }

    // ----- Accueil ---------------------------------------------------------

    fn home_page(self: &Rc<Self>) -> adw::NavigationPage {
        let add = pill_button("Ajouter un dossier réseau");
        let ui = self.clone();
        add.connect_clicked(move |_| ui.nav.push(&ui.connect_page()));

        let title = gtk::Label::builder()
            .label("Mes dossiers réseau")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.append(&add);
        let spacer = gtk::Box::builder().height_request(12).build();
        content.append(&spacer);
        content.append(&title);
        content.append(&self.home_list);
        content.append(&self.home_empty);

        let clamp = adw::Clamp::builder().maximum_size(560).child(&content).build();
        let status = adw::StatusPage::builder()
            .icon_name("folder-remote-symbolic")
            .title("NetFolders")
            .description("Accédez aux dossiers partagés de votre NAS comme s'ils se trouvaient sur cet ordinateur.")
            .child(&clamp)
            .build();

        let header = adw::HeaderBar::new();
        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_tooltip_text(Some("Actualiser"));
        let ui = self.clone();
        refresh.connect_clicked(move |_| ui.refresh_home());
        header.pack_start(&refresh);

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&status));
        adw::NavigationPage::builder()
            .title("NetFolders")
            .tag("home")
            .child(&tv)
            .build()
    }

    fn refresh_home(self: &Rc<Self>) {
        self.home_list.remove_all();
        let list = mounts::list();
        self.home_list.set_visible(!list.is_empty());
        self.home_empty.set_visible(list.is_empty());
        for m in list {
            self.home_list.append(&self.mount_row(m));
        }
    }

    fn mount_row(self: &Rc<Self>, m: NetMount) -> adw::ActionRow {
        let name = m
            .mount_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| m.mount_dir.display().to_string());
        let state = match (m.mounted, m.persistent) {
            (true, true) => "Connecté · automatique au démarrage",
            (true, false) => "Connecté · jusqu'au prochain redémarrage",
            (false, true) => "Se connecte à l'ouverture",
            (false, false) => "Déconnecté",
        };
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&name).as_str())
            .subtitle(glib::markup_escape_text(&format!("{}\n{} → {}", state, m.source, m.mount_dir.display())).as_str())
            .build();
        let ic = icon("folder-remote-symbolic");
        if m.mounted {
            ic.add_css_class("success");
        }
        row.add_prefix(&ic);

        let open = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Ouvrir dans Fichiers")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let ui = self.clone();
        let dir = m.mount_dir.clone();
        open.connect_clicked(move |_| ui.open_folder(&dir));
        row.add_suffix(&open);
        row.set_activatable_widget(Some(&open));

        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Modifier…")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let ui = self.clone();
        let m2 = m.clone();
        edit.connect_clicked(move |_| ui.nav.push(&ui.edit_page(m2.clone())));
        row.add_suffix(&edit);

        let eject = gtk::Button::builder()
            .icon_name("media-eject-symbolic")
            .tooltip_text("Déconnecter ou supprimer…")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let ui = self.clone();
        eject.connect_clicked(move |_| ui.confirm_unmount(&m, &name));
        row.add_suffix(&eject);
        row
    }

    fn confirm_unmount(self: &Rc<Self>, m: &NetMount, name: &str) {
        let body = if m.persistent {
            "Ce dossier se reconnecte automatiquement à chaque démarrage.\n\n\
             • « Déconnecter » le déconnecte jusqu'à la prochaine ouverture ou au prochain démarrage.\n\
             • « Supprimer » le retire définitivement de cet ordinateur, avec le mot de passe enregistré.\n\n\
             Les fichiers présents sur le NAS ne sont jamais supprimés."
        } else {
            "Le dossier ne sera plus accessible depuis cet ordinateur.\n\n\
             Les fichiers présents sur le NAS ne sont pas supprimés."
        };
        let d = adw::AlertDialog::new(Some(&format!("Déconnecter « {name} » ?")), Some(body));
        d.add_response("cancel", "Annuler");
        if m.persistent {
            if m.mounted {
                d.add_response("unmount", "Déconnecter");
            }
            d.add_response("forget", "Supprimer");
            d.set_response_appearance("forget", adw::ResponseAppearance::Destructive);
        } else {
            d.add_response("forget", "Déconnecter");
            d.set_response_appearance("forget", adw::ResponseAppearance::Destructive);
        }
        d.set_close_response("cancel");
        let ui = self.clone();
        let dir = m.mount_dir.clone();
        d.connect_response(None, move |_, resp| {
            let forget = match resp {
                "forget" => true,
                "unmount" => false,
                _ => return,
            };
            let ui = ui.clone();
            let dir = dir.clone();
            let mount_dir = dir.to_string_lossy().into_owned();
            run_bg(
                move || privileged::run(&Request::Unmount { mount_dir, forget }),
                move |r| {
                    match r {
                        Ok(msg) => {
                            if forget {
                                bookmarks::remove(&dir);
                                // Supprime le dossier local s'il est vide (jamais son contenu).
                                let _ = std::fs::remove_dir(&dir);
                            }
                            ui.toast(&msg);
                        }
                        Err(e) => ui.alert("Impossible de déconnecter", &e),
                    }
                    ui.refresh_home();
                },
            );
        });
        d.present(Some(&self.window));
    }

    // ----- Modification d'un dossier existant ------------------------------

    fn edit_page(self: &Rc<Self>, m: NetMount) -> adw::NavigationPage {
        let old_dir = m.mount_dir.clone();
        let parent = old_dir.parent().map(Path::to_path_buf).unwrap_or_else(home_dir);
        let old_name = old_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

        // Dossier sur le NAS
        let source = adw::EntryRow::builder().title("Adresse du dossier réseau").text(m.source.as_str()).build();
        let remote = adw::PreferencesGroup::builder()
            .title("Dossier sur le NAS")
            .description(
                "Format : //adresse-du-NAS/partage/sous-dossier. Modifiez l'adresse si \
                 l'adresse IP du NAS a changé, par exemple.",
            )
            .build();
        remote.add(&source);

        // Dossier local
        let name = adw::EntryRow::builder().title("Nom du dossier sur cet ordinateur").text(old_name.as_str()).build();
        let location = adw::ActionRow::builder()
            .title("Emplacement")
            .subtitle(glib::markup_escape_text(&old_dir.display().to_string()).as_str())
            .css_classes(["property"])
            .build();
        let (p, loc) = (parent.clone(), location.clone());
        name.connect_changed(move |e| {
            loc.set_subtitle(glib::markup_escape_text(&p.join(e.text().trim()).display().to_string()).as_str());
        });
        let local = adw::PreferencesGroup::builder().title("Sur cet ordinateur").build();
        local.add(&name);
        local.add(&location);

        // Identifiants
        let can_keep = m.has_saved_credentials() || m.is_guest();
        let current = if m.is_guest() {
            "Actuellement : accès invité (sans compte).".to_string()
        } else if let Some(u) = m.username() {
            format!("Actuellement : compte « {u} ».")
        } else {
            "Actuellement : identifiants enregistrés sur cet ordinateur.".to_string()
        };
        let change = adw::SwitchRow::builder()
            .title("Changer les identifiants")
            .subtitle(if can_keep {
                format!("{current} Laissez désactivé pour les conserver.")
            } else {
                "Le mot de passe actuel n'est pas enregistré sur cet ordinateur : saisissez-le à nouveau.".to_string()
            })
            .active(!can_keep)
            .sensitive(can_keep)
            .build();
        let need_auth = adw::SwitchRow::builder()
            .title("Le NAS demande un identifiant")
            .subtitle("Désactivez pour un accès invité.")
            .active(!m.is_guest())
            .build();
        let user = adw::EntryRow::builder().title("Nom d'utilisateur").text(m.username().unwrap_or_default()).build();
        let pass = adw::PasswordEntryRow::builder().title("Mot de passe").build();
        change.bind_property("active", &need_auth, "visible").sync_create().build();
        for w in [user.upcast_ref::<gtk::Widget>(), pass.upcast_ref()] {
            change.bind_property("active", w, "visible").sync_create().build();
            need_auth.bind_property("active", w, "sensitive").sync_create().build();
        }
        let auth = adw::PreferencesGroup::builder().title("Votre compte sur le NAS").build();
        auth.add(&change);
        auth.add(&need_auth);
        auth.add(&user);
        auth.add(&pass);

        // Options
        let persist = adw::SwitchRow::builder()
            .title("Reconnecter automatiquement au démarrage")
            .subtitle("Le dossier est enregistré dans /etc/fstab et reste disponible après chaque redémarrage.")
            .active(m.persistent)
            .build();
        let bookmark = adw::SwitchRow::builder()
            .title("Raccourci dans Fichiers")
            .active(bookmarks::contains(&old_dir))
            .build();
        let options = adw::PreferencesGroup::builder()
            .title("Options")
            .description(
                "Les modifications sont appliquées dès que vous cliquez sur « Enregistrer » : \
                 le dossier est déconnecté puis reconnecté avec les nouveaux réglages. Fermez \
                 d'abord les fichiers ouverts depuis ce dossier. Si la reconnexion échoue, \
                 l'ancienne configuration est restaurée.",
            )
            .build();
        options.add(&persist);
        options.add(&bookmark);

        let prefs = adw::PreferencesPage::new();
        prefs.add(&remote);
        prefs.add(&local);
        prefs.add(&auth);
        prefs.add(&options);

        const LABEL: &str = "Enregistrer les modifications";
        let btn = pill_button(LABEL);
        let ui = self.clone();
        btn.connect_clicked(move |btn| {
            let new_source = source.text().trim().trim_end_matches('/').to_string();
            if !new_source.starts_with("//") || new_source[2..].split('/').filter(|s| !s.is_empty()).count() < 2 {
                ui.alert("Adresse invalide", "L'adresse doit avoir la forme //adresse-du-NAS/partage (par exemple //192.168.1.20/Photos).");
                return;
            }
            let folder = name.text().trim().to_string();
            if !valid_folder_name(&folder) {
                ui.alert("Nom invalide", "Choisissez un nom de dossier simple, sans « / ».");
                return;
            }
            let credentials = if !change.is_active() {
                CredChoice::Keep
            } else if !need_auth.is_active() {
                CredChoice::Guest
            } else if user.text().trim().is_empty() {
                ui.alert("Nom d'utilisateur manquant", "Saisissez votre nom d'utilisateur, ou désactivez « Le NAS demande un identifiant ».");
                return;
            } else {
                CredChoice::User { username: user.text().trim().to_string(), password: pass.text().to_string() }
            };

            let new_dir = parent.join(&folder);
            let unchanged = new_source == m.source
                && new_dir == old_dir
                && matches!(credentials, CredChoice::Keep)
                && persist.is_active() == m.persistent;
            let set_bookmark = |ui: &Rc<Ui>| {
                bookmarks::remove(&old_dir);
                if bookmark.is_active() {
                    bookmarks::add(&new_dir, &folder);
                }
                ui.refresh_home();
            };
            if unchanged {
                // Seul le raccourci a pu changer : pas besoin des droits administrateur.
                set_bookmark(&ui);
                ui.nav.pop();
                ui.toast("Modifications enregistrées.");
                return;
            }
            if new_dir != old_dir {
                if let Err((title, body)) = prepare_local_dir(&new_dir) {
                    ui.alert(title, &body);
                    return;
                }
            }

            let req = Request::Modify {
                old_mount_dir: old_dir.to_string_lossy().into_owned(),
                source: new_source,
                mount_dir: new_dir.to_string_lossy().into_owned(),
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                credentials,
                persist: persist.is_active(),
            };
            set_busy(btn, true, "Application des modifications…");
            let (ui, btn) = (ui.clone(), btn.clone());
            let (old_dir, new_dir, folder, add_bookmark) = (old_dir.clone(), new_dir.clone(), folder.clone(), bookmark.is_active());
            run_bg(
                move || privileged::run(&req),
                move |r| {
                    set_busy(&btn, false, LABEL);
                    match r {
                        Ok(_) => {
                            bookmarks::remove(&old_dir);
                            if add_bookmark {
                                bookmarks::add(&new_dir, &folder);
                            }
                            if new_dir != old_dir {
                                // L'ancien dossier local est supprimé seulement s'il est vide.
                                let _ = std::fs::remove_dir(&old_dir);
                            }
                            ui.refresh_home();
                            ui.nav.pop();
                            ui.toast("Modifications enregistrées.");
                        }
                        Err(e) => {
                            if new_dir != old_dir {
                                let _ = std::fs::remove_dir(&new_dir);
                            }
                            ui.refresh_home();
                            ui.alert("Les modifications n'ont pas pu être appliquées", &e);
                        }
                    }
                },
            );
        });

        page(&format!("Modifier « {old_name} »"), None, &prefs, Some(&bottom_bar(&btn)))
    }

    // ----- Étape 1 : connexion --------------------------------------------

    fn connect_page(self: &Rc<Self>) -> adw::NavigationPage {
        let host = adw::EntryRow::builder().title("Adresse IP ou nom du NAS").build();
        let server = adw::PreferencesGroup::builder()
            .title("Où se trouve votre NAS ?")
            .description(
                "Par exemple 192.168.1.20 ou nas.local. Vous trouverez cette adresse dans \
                 l'application de votre NAS ou dans l'interface de votre box internet.",
            )
            .build();
        server.add(&host);

        let need_auth = adw::SwitchRow::builder()
            .title("Le NAS demande un identifiant")
            .subtitle("Désactivez si les partages sont accessibles sans compte (accès invité).")
            .active(true)
            .build();
        let user = adw::EntryRow::builder().title("Nom d'utilisateur").build();
        let pass = adw::PasswordEntryRow::builder().title("Mot de passe").build();
        need_auth.bind_property("active", &user, "sensitive").sync_create().build();
        need_auth.bind_property("active", &pass, "sensitive").sync_create().build();
        let auth = adw::PreferencesGroup::builder()
            .title("Votre compte sur le NAS")
            .description("Il s'agit du compte du NAS, pas de celui de cet ordinateur.")
            .build();
        auth.add(&need_auth);
        auth.add(&user);
        auth.add(&pass);

        let prefs = adw::PreferencesPage::new();
        prefs.add(&server);
        prefs.add(&auth);

        const LABEL: &str = "Rechercher les partages";
        let btn = pill_button(LABEL);
        btn.set_sensitive(false);
        let b = btn.clone();
        host.connect_changed(move |h| b.set_sensitive(!normalize_host(&h.text()).is_empty()));

        let ui = self.clone();
        let (h, u, p, a) = (host.clone(), user.clone(), pass.clone(), need_auth.clone());
        btn.connect_clicked(move |btn| {
            let host = normalize_host(&h.text());
            let user = u.text().trim().to_string();
            if a.is_active() && user.is_empty() {
                ui.alert("Nom d'utilisateur manquant", "Saisissez votre nom d'utilisateur, ou désactivez « Le NAS demande un identifiant ».");
                return;
            }
            let creds = Credentials {
                username: a.is_active().then_some(user),
                password: if a.is_active() { p.text().to_string() } else { String::new() },
            };
            set_busy(btn, true, "Connexion au NAS…");
            let (ui, btn) = (ui.clone(), btn.clone());
            let (h2, c2) = (host.clone(), creds.clone());
            run_bg(
                move || smb::list_shares(&h2, &c2),
                move |r| {
                    set_busy(&btn, false, LABEL);
                    match r {
                        Ok(shares) if shares.is_empty() => ui.alert(
                            "Aucun partage trouvé",
                            "Le NAS a répondu mais ne propose aucun dossier partagé à ce compte.",
                        ),
                        Ok(shares) => ui.nav.push(&ui.shares_page(&host, &creds, &shares)),
                        Err(e) => ui.alert("Connexion impossible", &e),
                    }
                },
            );
        });

        // Entrée = valider
        for entry in [host.upcast_ref::<adw::EntryRow>(), &user, pass.upcast_ref()] {
            let b = btn.clone();
            entry.connect_entry_activated(move |_| {
                if b.is_sensitive() {
                    b.emit_clicked();
                }
            });
        }

        let page = page("Connexion au NAS", Some(1), &prefs, Some(&bottom_bar(&btn)));
        let h = host.clone();
        page.connect_shown(move |_| {
            h.grab_focus();
        });
        page
    }

    // ----- Étape 2 : partages ---------------------------------------------

    fn shares_page(self: &Rc<Self>, host: &str, creds: &Credentials, shares: &[Share]) -> adw::NavigationPage {
        let group = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(&format!("Partages disponibles sur {host}")).as_str())
            .description("Cliquez sur le partage qui contient vos fichiers. Vous pourrez ensuite choisir un sous-dossier précis.")
            .build();
        for s in shares {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&s.name).as_str())
                .subtitle(glib::markup_escape_text(&s.comment).as_str())
                .activatable(true)
                .build();
            row.add_prefix(&icon("folder-remote-symbolic"));
            row.add_suffix(&icon("go-next-symbolic"));
            let ui = self.clone();
            let target = Target { host: host.to_string(), creds: creds.clone(), share: s.name.clone(), path: vec![] };
            row.connect_activated(move |_| ui.nav.push(&ui.folder_page(target.clone())));
            group.add(&row);
        }
        let prefs = adw::PreferencesPage::new();
        prefs.add(&group);
        page("Choix du partage", Some(2), &prefs, None)
    }

    // ----- Étape 3 : navigation dans les dossiers --------------------------

    fn folder_page(self: &Rc<Self>, t: Target) -> adw::NavigationPage {
        let current = adw::PreferencesGroup::builder().title("Dossier sélectionné").build();
        let cur_row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(t.name()).as_str())
            .subtitle(glib::markup_escape_text(&t.unc()).as_str())
            .build();
        cur_row.add_prefix(&icon("folder-symbolic"));
        current.add(&cur_row);

        let subdirs = adw::PreferencesGroup::builder()
            .title("Sous-dossiers")
            .description(
                "Cliquez sur un sous-dossier pour y entrer, ou sur « Choisir ce dossier » \
                 pour monter le dossier sélectionné. La flèche en haut à gauche permet de revenir en arrière.",
            )
            .build();
        let loading = adw::ActionRow::builder().title("Chargement…").build();
        let spinner = gtk::Spinner::new();
        spinner.start();
        loading.add_prefix(&spinner);
        subdirs.add(&loading);

        let prefs = adw::PreferencesPage::new();
        prefs.add(&current);
        prefs.add(&subdirs);

        let btn = pill_button("Choisir ce dossier");
        let ui = self.clone();
        let t2 = t.clone();
        btn.connect_clicked(move |_| ui.nav.push(&ui.options_page(t2.clone())));

        let ui = self.clone();
        let t3 = t.clone();
        run_bg(
            move || smb::list_dirs(&t3.host, &t3.share, &t3.path, &t3.creds),
            move |r| {
                subdirs.remove(&loading);
                match r {
                    Ok(dirs) if dirs.is_empty() => {
                        let row = adw::ActionRow::builder().title("Aucun sous-dossier").build();
                        row.add_css_class("dim-label");
                        subdirs.add(&row);
                    }
                    Ok(dirs) => {
                        for d in dirs {
                            let row = adw::ActionRow::builder()
                                .title(glib::markup_escape_text(&d).as_str())
                                .activatable(true)
                                .build();
                            row.add_prefix(&icon("folder-symbolic"));
                            row.add_suffix(&icon("go-next-symbolic"));
                            let ui = ui.clone();
                            let child = t.child(&d);
                            row.connect_activated(move |_| ui.nav.push(&ui.folder_page(child.clone())));
                            subdirs.add(&row);
                        }
                    }
                    Err(e) => {
                        let row = adw::ActionRow::builder()
                            .title("Impossible d'afficher le contenu")
                            .subtitle(glib::markup_escape_text(&e).as_str())
                            .build();
                        row.add_prefix(&icon("dialog-warning-symbolic"));
                        subdirs.add(&row);
                    }
                }
            },
        );

        page("Choix du dossier", Some(3), &prefs, Some(&bottom_bar(&btn)))
    }

    // ----- Étape 4 : options et montage -----------------------------------

    fn options_page(self: &Rc<Self>, t: Target) -> adw::NavigationPage {
        let home = home_dir();
        let name = adw::EntryRow::builder().title("Nom du dossier sur cet ordinateur").text(t.name()).build();
        let location = adw::ActionRow::builder()
            .title("Emplacement")
            .subtitle(glib::markup_escape_text(&home.join(t.name()).display().to_string()).as_str())
            .css_classes(["property"])
            .build();
        let h = home.clone();
        let loc = location.clone();
        name.connect_changed(move |e| {
            loc.set_subtitle(glib::markup_escape_text(&h.join(e.text().trim()).display().to_string()).as_str());
        });
        let local = adw::PreferencesGroup::builder()
            .title("Où l'afficher sur cet ordinateur ?")
            .description(
                glib::markup_escape_text(&format!(
                    "Le contenu de {} apparaîtra dans ce dossier, dans votre dossier personnel.",
                    t.unc()
                ))
                .as_str(),
            )
            .build();
        local.add(&name);
        local.add(&location);

        let persist = adw::SwitchRow::builder()
            .title("Reconnecter automatiquement au démarrage")
            .subtitle(
                "Recommandé. Le dossier sera toujours disponible sans relancer cette application. \
                 Le démarrage n'est pas bloqué si le NAS est éteint.",
            )
            .active(true)
            .build();
        let bookmark = adw::SwitchRow::builder()
            .title("Ajouter un raccourci dans Fichiers")
            .subtitle("Le dossier apparaîtra dans la barre latérale du gestionnaire de fichiers.")
            .active(true)
            .build();
        let mut note = String::from("Votre mot de passe administrateur vous sera demandé pour effectuer le montage.");
        if t.creds.username.is_some() {
            note.push_str(
                " Si la reconnexion automatique est activée, le mot de passe du NAS est enregistré \
                 dans un fichier lisible uniquement par l'administrateur.",
            );
        }
        let options = adw::PreferencesGroup::builder().title("Options").description(note.as_str()).build();
        options.add(&persist);
        options.add(&bookmark);

        let prefs = adw::PreferencesPage::new();
        prefs.add(&local);
        prefs.add(&options);

        const LABEL: &str = "Monter le dossier";
        let btn = pill_button(LABEL);
        let ui = self.clone();
        btn.connect_clicked(move |btn| {
            let folder = name.text().trim().to_string();
            if !valid_folder_name(&folder) {
                ui.alert("Nom invalide", "Choisissez un nom de dossier simple, sans « / ».");
                return;
            }
            let dir = home.join(&folder);
            if let Err((title, body)) = prepare_local_dir(&dir) {
                ui.alert(title, &body);
                return;
            }

            let req = Request::Mount {
                source: t.unc(),
                mount_dir: dir.to_string_lossy().into_owned(),
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                username: t.creds.username.clone(),
                password: t.creds.password.clone(),
                persist: persist.is_active(),
            };
            let add_bookmark = bookmark.is_active();
            set_busy(btn, true, "Montage en cours…");
            let (ui, btn, t) = (ui.clone(), btn.clone(), t.clone());
            run_bg(
                move || privileged::run(&req),
                move |r| {
                    set_busy(&btn, false, LABEL);
                    match r {
                        Ok(msg) => {
                            if add_bookmark {
                                bookmarks::add(&dir, &folder);
                            }
                            ui.refresh_home();
                            ui.nav.push(&ui.done_page(&t, &dir, &msg));
                        }
                        Err(e) => {
                            let _ = std::fs::remove_dir(&dir);
                            ui.alert("Le montage a échoué", &e);
                        }
                    }
                },
            );
        });

        page("Options", Some(4), &prefs, Some(&bottom_bar(&btn)))
    }

    // ----- Résultat --------------------------------------------------------

    fn done_page(self: &Rc<Self>, t: &Target, dir: &Path, msg: &str) -> adw::NavigationPage {
        let count = std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0);
        let description = format!(
            "<b>{}</b> est maintenant accessible dans :\n<b>{}</b>\n\n{} élément(s) trouvé(s).\n{}",
            glib::markup_escape_text(&t.unc()),
            glib::markup_escape_text(&dir.display().to_string()),
            count,
            glib::markup_escape_text(msg.lines().skip(1).collect::<Vec<_>>().join("\n").as_str()),
        );

        let open = pill_button("Ouvrir le dossier");
        let ui = self.clone();
        let d = dir.to_path_buf();
        open.connect_clicked(move |_| ui.open_folder(&d));
        let finish = gtk::Button::builder()
            .label("Terminer")
            .css_classes(["pill"])
            .halign(gtk::Align::Center)
            .build();
        let ui = self.clone();
        finish.connect_clicked(move |_| {
            ui.nav.pop_to_tag("home");
        });
        let buttons = gtk::Box::new(gtk::Orientation::Vertical, 12);
        buttons.append(&open);
        buttons.append(&finish);

        let status = adw::StatusPage::builder()
            .icon_name("object-select-symbolic")
            .title("C'est prêt !")
            .description(description.as_str())
            .child(&buttons)
            .build();
        let header = adw::HeaderBar::new();
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&status));
        adw::NavigationPage::builder()
            .title("Terminé")
            .can_pop(false)
            .child(&tv)
            .build()
    }
}
