//! «Подключение к серверу»: saved connections, the login form (SFTP / FTP / FTPS),
//! the host-key question and what the rest of the window needs to know about remote
//! folders.
use super::*;
use crate::config::Connection;
use quokka_domain::remote::{self, Protocol, Site};
use quokka_infra::creds;
use quokka_infra::net::{self, ConnectError, Login};
use quokka_infra::ssh_term;
use std::sync::mpsc::{self, Receiver};

pub(super) struct ConnectDialog {
    /// The saved connection being edited; `None` — a new one.
    selected: Option<usize>,
    name: String,
    protocol: Protocol,
    host: String,
    port: String,
    user: String,
    use_key: bool,
    /// The password, or the key passphrase when `use_key`.
    secret: String,
    key_file: String,
    /// Keep `secret` in the Windows Credential Manager.
    remember: bool,
    /// Folder to open once connected (instead of the home folder).
    target: Option<PathBuf>,
    error: Option<String>,
    note: Option<String>,
    /// Fingerprint of an unknown server key, waiting for the user's yes.
    unknown_key: Option<String>,
    busy: Option<Receiver<Result<PathBuf, ConnectError>>>,
    /// Connect as soon as the dialog shows (a saved login with its password).
    auto_start: bool,
    first: bool,
    focus_secret: bool,
    focus_name: bool,
    /// Saved connection whose row asks «delete?».
    confirm_delete: Option<usize>,
}

/// What a click in a saved-connection row asks for.
enum RowAction {
    None,
    Select,
    Edit,
    Connect,
    Delete,
    Terminal,
}

pub(super) enum Outcome {
    Open,
    Cancel,
    Connected { site: Site, home: PathBuf, target: Option<PathBuf> },
    /// Store the form as a saved connection (`selected`, or a new one).
    Save,
    Delete(usize),
    /// Open an SSH terminal for saved connection `index`.
    Terminal(usize),
}

impl ConnectDialog {
    pub fn new() -> Self {
        ConnectDialog {
            selected: None,
            name: String::new(),
            protocol: Protocol::Sftp,
            host: String::new(),
            port: Protocol::Sftp.default_port().to_string(),
            user: String::new(),
            use_key: false,
            secret: String::new(),
            key_file: String::new(),
            remember: false,
            target: None,
            error: None,
            note: None,
            unknown_key: None,
            busy: None,
            auto_start: false,
            first: true,
            focus_secret: false,
            focus_name: false,
            confirm_delete: None,
        }
    }

    /// One saved connection: protocol and name, then «terminal · connect · edit · delete»
    /// on the right (the terminal only for SFTP). Delete asks once more in the same row.
    fn saved_row(&mut self, ui: &mut Ui, i: usize, c: &Connection, busy: bool) -> RowAction {
        const BTN: f32 = 26.0;
        const GAP: f32 = 2.0;
        let confirming = self.confirm_delete == Some(i);
        let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), if busy { Sense::hover() } else { Sense::click() });
        let row = row.on_hover_text(&c.url);
        let hovered = !busy && ui.rect_contains_pointer(rect);
        let on = self.selected == Some(i);
        let p = ui.painter();
        if confirming {
            p.rect_filled(rect, rad(5.0), DANGER_SOFT());
        } else if on {
            p.rect_filled(rect, rad(5.0), BLUE_SOFT());
        } else if hovered {
            p.rect_filled(rect, rad(5.0), INPUT_BG());
        }

        let buttons_w = 4.0 * BTN + 3.0 * GAP;
        let text_right = rect.right() - buttons_w - 8.0;
        let text_left = rect.left() + 8.0;
        let text_col = if busy { MUTED() } else { TEXT() };
        if confirming {
            let g = p.layout_no_wrap(tr!("Удалить?", "Delete?").to_string(), semibold(13.5), RED);
            p.galley(pos2(text_left, rect.center().y - g.size().y / 2.0), g, RED);
        } else {
            let proto = Site::parse_url(&c.url).map(|s| s.protocol.label()).unwrap_or("?");
            let pg = p.layout_no_wrap(proto.to_string(), semibold(11.5), MUTED());
            let pw = pg.size().x;
            p.galley(pos2(text_left, rect.center().y - pg.size().y / 2.0), pg, MUTED());
            let name_left = text_left + pw.max(34.0) + 6.0;
            let mut job = egui::text::LayoutJob::simple_singleline(c.title().to_string(), regular(13.5), text_col);
            job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - name_left).max(10.0));
            let g = ui.fonts_mut(|f| f.layout_job(job));
            p.galley(pos2(name_left, rect.center().y - g.size().y / 2.0), g, text_col);
        }

        let mut act = RowAction::None;
        if row.clicked() && !confirming {
            act = RowAction::Select;
        }
        if row.double_clicked() && !confirming {
            act = RowAction::Connect;
        }
        let x0 = rect.right() - buttons_w - 2.0;
        let button = |ui: &mut Ui, slot: u8, k: u8, g: Glyph, col: Color32, tip: &str| {
            let x = x0 + f32::from(slot) * (BTN + GAP);
            let r = Rect::from_min_size(pos2(x, rect.center().y - BTN / 2.0), vec2(BTN, BTN));
            row_button(ui, r, ui.scope_id().with(("saved-conn", i, k)), g, col, !busy, tip)
        };
        if confirming {
            // two buttons in the slots of «edit» and «delete»
            if button(ui, 2, 3, Glyph::Check, RED, tr!("Да, удалить", "Yes, delete")).clicked() {
                self.confirm_delete = None;
                act = RowAction::Delete;
            }
            if button(ui, 3, 4, Glyph::Close, TEXT_SOFT(), tr!("Не удалять", "Keep it")).clicked() {
                self.confirm_delete = None;
            }
        } else {
            let sftp = Site::parse_url(&c.url).is_some_and(|s| s.protocol == Protocol::Sftp);
            if sftp && button(ui, 0, 5, Glyph::Terminal, TEXT_SOFT(), tr!("SSH-терминал (PuTTY / ssh)", "SSH terminal (PuTTY / ssh)")).clicked() {
                act = RowAction::Terminal;
            }
            if button(ui, 1, 0, Glyph::ArrowRight, BLUE(), tr!("Подключиться", "Connect")).clicked() {
                act = RowAction::Connect;
            }
            if button(ui, 2, 1, Glyph::Pencil, TEXT_SOFT(), tr!("Изменить", "Edit")).clicked() {
                act = RowAction::Edit;
            }
            if button(ui, 3, 2, Glyph::Trash, RED, tr!("Удалить", "Delete")).clicked() {
                self.confirm_delete = Some(i);
            }
        }
        act
    }

    /// Open `path` on a server we are not logged in to: use its saved connection if
    /// there is one (and log straight in when its password is saved).
    pub fn for_path(path: &std::path::Path, saved: &[Connection]) -> Self {
        let mut d = ConnectDialog::new();
        if let Some((site, _)) = remote::split(path) {
            match saved.iter().position(|c| Site::parse_url(&c.url).as_ref() == Some(&site)) {
                Some(i) => {
                    d.load(i, &saved[i]);
                    d.auto_start = !d.secret.is_empty() || (d.use_key && !saved[i].save_password);
                }
                None => d.fill(&site),
            }
            d.target = Some(path.to_path_buf());
        }
        d
    }

    fn fill(&mut self, site: &Site) {
        self.protocol = site.protocol;
        self.host = site.host.clone();
        self.port = site.port.to_string();
        self.user = site.user.clone();
        if site.protocol != Protocol::Sftp {
            self.use_key = false;
        }
    }

    /// Put saved connection `i` into the form.
    fn load(&mut self, i: usize, c: &Connection) {
        *self = ConnectDialog { target: self.target.take(), first: false, ..ConnectDialog::new() };
        self.selected = Some(i);
        self.name = c.name.clone();
        if let Some(site) = Site::parse_url(&c.url) {
            self.fill(&site);
        }
        self.key_file = c.key_file.clone();
        self.use_key = !c.key_file.is_empty() && self.protocol == Protocol::Sftp;
        self.remember = c.save_password;
        if c.save_password {
            self.secret = creds::load(&c.url).unwrap_or_default();
        }
        self.focus_secret = self.secret.is_empty();
    }

    fn site(&self) -> Result<Site, String> {
        let mut protocol = self.protocol;
        let mut host = self.host.trim().trim_end_matches('/').to_string();
        let mut user = self.user.trim().to_string();
        let mut port = self.port.trim().to_string();
        // a whole address typed into the host field fills the other fields
        if let Some((scheme, rest)) = host.clone().split_once("://") {
            protocol = Protocol::from_scheme(scheme).ok_or_else(|| trf!("Неизвестный протокол «{scheme}»", "Unknown protocol “{scheme}”"))?;
            host = rest.split('/').next().unwrap_or("").to_string();
        }
        if let Some(site) = host.contains('@').then(|| Site::parse(protocol, &host)).flatten() {
            user = site.user;
            port = site.port.to_string();
            host = site.host;
        } else if let Some((h, p)) = host.rsplit_once(':').filter(|(h, p)| !h.contains(':') && p.parse::<u16>().is_ok()) {
            (port, host) = (p.to_string(), h.to_string());
        }
        if host.is_empty() {
            return Err(tr!("Укажите сервер", "Enter the server").into());
        }
        if user.is_empty() {
            if protocol == Protocol::Sftp {
                return Err(tr!("Укажите имя пользователя", "Enter the user name").into());
            }
            user = "anonymous".into();
        }
        let port = port.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| tr!("Неверный порт", "Invalid port").to_string())?;
        Ok(Site { protocol, user, host, port })
    }

    /// The form as a saved connection.
    pub fn profile(&self) -> Result<Connection, String> {
        let site = self.site()?;
        let use_key = self.use_key && site.protocol == Protocol::Sftp;
        if use_key && self.key_file.trim().is_empty() {
            return Err(tr!("Укажите файл ключа", "Choose the key file").into());
        }
        Ok(Connection {
            name: self.name.trim().to_string(),
            url: site.url(),
            key_file: if use_key { self.key_file.trim().to_string() } else { String::new() },
            save_password: self.remember,
        })
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    fn start(&mut self, trust: Option<String>, waker: Waker) {
        let (site, profile) = match self.site().and_then(|s| self.profile().map(|p| (s, p))) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let secret = (!self.secret.is_empty()).then(|| self.secret.clone());
        let login = if !profile.key_file.is_empty() {
            Login { password: None, key_file: Some(PathBuf::from(&profile.key_file)), passphrase: secret }
        } else {
            // an empty SFTP password still lets the usual ~/.ssh keys be tried
            Login { password: secret, key_file: None, passphrase: None }
        };
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("quokka-connect".into())
            .spawn(move || {
                let _ = tx.send(net::connect(&site, login, trust));
                waker.wake();
            })
            .ok();
        self.error = None;
        self.note = None;
        self.busy = Some(rx);
    }

    fn set_protocol(&mut self, p: Protocol) {
        if self.port.trim().parse::<u16>().ok() == Some(self.protocol.default_port()) || self.port.trim().is_empty() {
            self.port = p.default_port().to_string();
        }
        self.protocol = p;
        if p != Protocol::Sftp {
            self.use_key = false;
        }
    }

    /// Saved connection `index` is gone: clear the form if it was the one in it,
    /// otherwise keep the form and shift its index.
    pub fn removed(&mut self, index: usize) {
        match self.selected {
            Some(s) if s == index => *self = ConnectDialog { target: self.target.take(), ..ConnectDialog::new() },
            Some(s) if s > index => self.selected = Some(s - 1),
            _ => {}
        }
        self.confirm_delete = None;
    }

    pub fn saved(&mut self, index: usize) {
        self.selected = Some(index);
        self.note = Some(tr!("Подключение сохранено", "Connection saved").into());
        self.error = None;
    }

    pub fn show(&mut self, ctx: &egui::Context, esc: bool, enter: bool, saved: &[Connection], hwnd: isize) -> Outcome {
        if let Some(rx) = &self.busy {
            match rx.try_recv() {
                Ok(Ok(home)) => {
                    let site = self.site().expect("validated before connecting");
                    return Outcome::Connected { site, home, target: self.target.clone() };
                }
                Ok(Err(ConnectError::UnknownHost { fingerprint })) => {
                    self.busy = None;
                    self.unknown_key = Some(fingerprint);
                }
                Ok(Err(e)) => {
                    self.busy = None;
                    self.error = Some(e.to_string());
                    self.focus_secret = true;
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.busy = None;
                    self.error = Some(tr!("Подключение прервано", "Connection aborted").into());
                }
            }
        }
        let waker = waker(ctx);
        if std::mem::take(&mut self.auto_start) {
            self.start(None, waker.clone());
        }
        let busy = self.busy.is_some();
        let mut submit = enter && !busy;
        let mut cancel = esc;
        let mut trust = false;
        let mut outcome = Outcome::Open;

        let width = 840.0_f32.min(ctx.content_rect().width() - 48.0);
        dialog(ctx, "server-connect", width, |ui| {
            dialog_header(ui, Glyph::Cloud, BLUE(), tr!("Подключение к серверу", "Connect to a server"),
                Some(tr!("SFTP, FTP и FTPS · сохранённые подключения слева", "SFTP, FTP and FTPS · saved connections on the left")));

            if let Some(fp) = self.unknown_key.clone() {
                let site = self.site().map(|s| s.authority()).unwrap_or_default();
                ui.label(RichText::new(trf!("Сервер {site} подключается впервые.", "First connection to {site}.")).color(TEXT()));
                ui.add_space(6.0);
                ui.label(RichText::new(tr!("Отпечаток ключа сервера:", "Server key fingerprint:")).color(MUTED()).size(13.0));
                ui.label(RichText::new(&fp).monospace().color(TEXT()));
                ui.add_space(6.0);
                ui.label(RichText::new(tr!(
                    "Сверьте его с отпечатком, который даёт администратор сервера. Ключ будет записан в ~/.ssh/known_hosts.",
                    "Compare it with the fingerprint from the server administrator. The key will be saved to ~/.ssh/known_hosts.")).color(MUTED()).size(13.0));
                ui.add_space(14.0);
                buttons_row(ui, |ui| {
                    if btn(ui, tr!("Доверять и подключить", "Trust and connect"), BLUE(), Color32::WHITE).clicked() {
                        trust = true;
                    }
                    if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG(), TEXT()).clicked() {
                        self.unknown_key = None;
                    }
                });
                submit = false;
                cancel = false;
                if enter {
                    trust = true;
                }
                if esc {
                    self.unknown_key = None;
                }
                return;
            }

            ui.horizontal_top(|ui| {
                // ── saved connections ──
                ui.vertical(|ui| {
                    ui.set_width(290.0);
                    if menu_item_icon(ui, Some(Glyph::Plus), tr!("Новое подключение", "New connection"), "", !busy, false).clicked() {
                        let target = self.target.take();
                        *self = ConnectDialog { target, ..ConnectDialog::new() };
                    }
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().id_salt("saved-connections").max_height(300.0).show(ui, |ui| {
                        if saved.is_empty() {
                            ui.label(RichText::new(tr!("Сохранённых пока нет", "Nothing saved yet")).color(MUTED()).size(13.0));
                        }
                        for (i, c) in saved.iter().enumerate() {
                            match self.saved_row(ui, i, c, busy) {
                                RowAction::None => {}
                                RowAction::Select => self.load(i, c),
                                RowAction::Edit => {
                                    self.load(i, c);
                                    self.focus_name = true;
                                    self.focus_secret = false;
                                }
                                RowAction::Connect => {
                                    self.load(i, c);
                                    submit = true;
                                }
                                RowAction::Delete => outcome = Outcome::Delete(i),
                                RowAction::Terminal => outcome = Outcome::Terminal(i),
                            }
                        }
                    });
                });
                ui.separator();

                // ── the form ──
                ui.vertical(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        egui::Grid::new("server-fields").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                            let field = |ui: &mut Ui, text: &mut String, hint: &str, password: bool, width: f32| {
                                ui.add(egui::TextEdit::singleline(text).password(password).hint_text(hint)
                                    .desired_width(width).margin(vec2(8.0, 6.0)).font(regular(14.5)))
                            };
                            ui.label(RichText::new(tr!("Название", "Name")).color(TEXT()));
                            let r = field(ui, &mut self.name, tr!("необязательно", "optional"), false, 380.0);
                            if std::mem::take(&mut self.focus_name) {
                                r.request_focus();
                            }
                            ui.end_row();

                            ui.label(RichText::new(tr!("Протокол", "Protocol")).color(TEXT()));
                            let labels: Vec<&str> = Protocol::ALL.iter().map(|p| p.label()).collect();
                            let cur = Protocol::ALL.iter().position(|p| *p == self.protocol).unwrap_or(0);
                            if let Some(i) = segmented(ui, &labels, cur) {
                                self.set_protocol(Protocol::ALL[i]);
                            }
                            ui.end_row();

                            ui.label(RichText::new(tr!("Сервер", "Server")).color(TEXT()));
                            ui.horizontal(|ui| {
                                let r = field(ui, &mut self.host, "example.com", false, 250.0);
                                if self.first {
                                    self.first = false;
                                    if self.host.is_empty() { r.request_focus(); } else { self.focus_secret = true; }
                                }
                                ui.label(RichText::new(tr!("Порт", "Port")).color(TEXT()));
                                field(ui, &mut self.port, "22", false, 64.0);
                            });
                            ui.end_row();

                            ui.label(RichText::new(tr!("Пользователь", "User")).color(TEXT()));
                            let hint = if self.protocol == Protocol::Sftp { "root" } else { "anonymous" };
                            field(ui, &mut self.user, hint, false, 380.0);
                            ui.end_row();

                            if self.protocol == Protocol::Sftp {
                                ui.label("");
                                if let Some(i) = segmented(ui, &[tr!("Пароль", "Password"), tr!("Файл ключа", "Key file")], usize::from(self.use_key)) {
                                    self.use_key = i == 1;
                                }
                                ui.end_row();
                            }
                            if self.use_key {
                                ui.label(RichText::new(tr!("Ключ", "Key")).color(TEXT()));
                                ui.horizontal(|ui| {
                                    field(ui, &mut self.key_file, r"C:\Users\…\.ssh\id_ed25519", false, 280.0);
                                    if btn(ui, tr!("Обзор…", "Browse…"), INPUT_BG(), TEXT()).clicked() {
                                        let start = (!self.key_file.trim().is_empty()).then(|| PathBuf::from(self.key_file.trim()));
                                        if let Some(p) = quokka_infra::pick::key_file(hwnd, start.as_deref()) {
                                            self.key_file = p.display().to_string();
                                            self.focus_secret = true;
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                            let (label, hint) = if self.use_key {
                                (tr!("Пароль ключа", "Passphrase"), tr!("если ключ зашифрован", "if the key is encrypted"))
                            } else if self.protocol == Protocol::Sftp {
                                (tr!("Пароль", "Password"), tr!("пусто — ключи из ~/.ssh", "empty — keys from ~/.ssh"))
                            } else {
                                (tr!("Пароль", "Password"), "")
                            };
                            ui.label(RichText::new(label).color(TEXT()));
                            let r = field(ui, &mut self.secret, hint, true, 380.0);
                            if self.focus_secret {
                                self.focus_secret = false;
                                r.request_focus();
                            }
                            ui.end_row();

                            ui.label("");
                            ui.checkbox(&mut self.remember, if self.use_key {
                                tr!("Запомнить пароль ключа", "Remember the passphrase")
                            } else {
                                tr!("Запомнить пароль", "Remember the password")
                            }).on_hover_text(tr!(
                                "Пароль хранится в диспетчере учётных данных Windows, зашифрованным для вашей учётной записи. Без этой отметки он забывается при закрытии Quokka.",
                                "The password is kept in the Windows Credential Manager, encrypted for your account. Without this it is forgotten when Quokka closes."));
                            ui.end_row();
                        });
                    });

                    if let Some(e) = &self.error {
                        ui.add_space(8.0);
                        ui.label(RichText::new(e).color(RED));
                    } else if let Some(n) = &self.note {
                        ui.add_space(8.0);
                        ui.label(RichText::new(n).color(GREEN));
                    }
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if busy {
                            ui.add(egui::Spinner::new());
                            ui.label(RichText::new(tr!("Подключение…", "Connecting…")).color(MUTED()));
                        } else {
                            if btn(ui, tr!("Подключить", "Connect"), BLUE(), Color32::WHITE).clicked() {
                                submit = true;
                            }
                            if btn(ui, tr!("Сохранить", "Save"), INPUT_BG(), TEXT()).clicked() {
                                outcome = Outcome::Save;
                            }
                            if let Some(i) = self.selected {
                                if btn(ui, tr!("Удалить", "Delete"), INPUT_BG(), RED).clicked() {
                                    outcome = Outcome::Delete(i);
                                }
                            }
                        }
                        if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG(), TEXT()).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        });

        if cancel {
            // a login still in flight finishes in the background and is simply kept
            return Outcome::Cancel;
        }
        if trust {
            let fp = self.unknown_key.take();
            self.start(fp, waker);
        } else if submit {
            self.start(None, waker);
        }
        if matches!(outcome, Outcome::Save) {
            if let Err(e) = self.profile() {
                self.error = Some(e);
                return Outcome::Open;
            }
        }
        outcome
    }
}

/// Icon button laid over a saved-connection row.
fn row_button(ui: &mut Ui, r: Rect, id: Id, g: Glyph, color: Color32, enabled: bool, tip: &str) -> Response {
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(r, rad(5.0), TRACK());
    }
    let col = if enabled { color } else { DISABLED() };
    icons::glyph(ui.painter(), r.shrink(6.0), g, col);
    resp.on_hover_text(tip)
}

impl QuokkaApp {
    pub(super) fn open_connect(&mut self, path: Option<&std::path::Path>) {
        self.modal = Modal::Connect(Box::new(match path {
            Some(p) => ConnectDialog::for_path(p, &self.settings.connections),
            None => ConnectDialog::new(),
        }));
    }

    /// Store the dialog's form as a saved connection, and its password if asked to.
    /// Returns the index it got.
    fn save_connection(&mut self, d: &ConnectDialog) -> Result<usize, String> {
        let profile = d.profile()?;
        let user = Site::parse_url(&profile.url).map(|s| s.user).unwrap_or_default();
        // the old entry may have had another address: its password goes with it
        if let Some(old) = d.selected().and_then(|i| self.settings.connections.get(i)) {
            if old.url != profile.url {
                let _ = creds::delete(&old.url);
            }
        }
        if profile.save_password && !d.secret().is_empty() {
            creds::save(&profile.url, &user, d.secret()).map_err(|e| trf!("Не удалось сохранить пароль: {e}", "Could not save the password: {e}"))?;
        } else if !profile.save_password {
            let _ = creds::delete(&profile.url);
        }
        let list = &mut self.settings.connections;
        let index = match d.selected().filter(|i| *i < list.len()).or_else(|| list.iter().position(|c| c.url == profile.url)) {
            Some(i) => {
                list[i] = profile;
                i
            }
            None => {
                list.push(profile);
                list.len() - 1
            }
        };
        self.save_settings();
        Ok(index)
    }

    pub(super) fn connect_ui(&mut self, ctx: &egui::Context, d: &mut ConnectDialog, esc: bool, enter: bool) -> bool {
        let saved = self.settings.connections.clone();
        match d.show(ctx, esc, enter, &saved, self.hwnd) {
            Outcome::Open => true,
            Outcome::Cancel => false,
            Outcome::Save => {
                match self.save_connection(d) {
                    Ok(i) => d.saved(i),
                    Err(e) => self.toast(e, true),
                }
                true
            }
            Outcome::Terminal(i) => {
                if let Some(site) = saved.get(i).and_then(|c| Site::parse_url(&c.url)) {
                    self.open_ssh_terminal(&site, None);
                }
                true
            }
            Outcome::Delete(i) => {
                if let Some(c) = self.settings.connections.get(i).cloned() {
                    let _ = creds::delete(&c.url);
                    self.settings.connections.remove(i);
                    self.save_settings();
                    self.toast(trf!("Подключение «{}» удалено", "Connection “{}” deleted", c.title()), false);
                }
                d.removed(i);
                true
            }
            Outcome::Connected { site, home, target } => {
                // a saved connection (or one whose password should be remembered) keeps its changes
                if d.selected().is_some() || d.profile().is_ok_and(|p| p.save_password) {
                    if let Err(e) = self.save_connection(d) {
                        self.toast(e, true);
                    }
                }
                let r = self.ap().navigate(target.unwrap_or(home));
                self.report(r);
                self.toast(trf!("Подключено: {}", "Connected: {}", site.url()), false);
                false
            }
        }
    }

    /// Open PuTTY (or `ssh.exe`) on `site`, in the remote folder `dir` when given;
    /// the key of its saved connection, if any, is passed along.
    pub(super) fn open_ssh_terminal(&mut self, site: &Site, dir: Option<&str>) {
        let key = self.settings.connections.iter()
            .find(|c| Site::parse_url(&c.url).as_ref() == Some(site))
            .map(|c| PathBuf::from(c.key_file.trim()));
        match ssh_term::open(site, dir, key.as_deref()) {
            Ok(client) => self.toast(trf!("SSH-терминал ({}): {}", "SSH terminal ({}): {}", client.label(), site.url()), false),
            Err(e) => self.toast(e.to_string(), true),
        }
    }

    /// Close the connection of the active tab's server and leave its folders.
    pub(super) fn disconnect_current(&mut self) {
        let path = self.ap_ref().path.clone();
        if remote::split(&path).is_none() {
            self.toast(tr!("Откройте папку на сервере, чтобы отключиться", "Open a server folder to disconnect"), true);
            return;
        }
        self.disconnect_from(&path);
    }

    /// Close the connection of the server `path` is on. Its tabs are closed; a pane
    /// that has nothing else keeps one tab, sent back to the last local folder of
    /// its history (or «This PC»). The server is also dropped from the histories.
    pub(super) fn disconnect_from(&mut self, path: &std::path::Path) {
        let Some((site, _)) = remote::split(path) else { return };
        net::disconnect(&site);
        let on_site = |p: &std::path::Path| remote::split(p).is_some_and(|(s, _)| s == site);
        for side in &mut self.sides {
            let keep: Vec<bool> = side.tabs.iter().map(|t| !on_site(&t.path)).collect();
            if keep.iter().all(|k| *k) {
                // nothing of this server here
            } else if keep.iter().any(|k| *k) {
                // as with closing a tab: a closed current tab hands over to its right neighbour
                let cur = side.cur;
                let next = (cur..keep.len()).find(|&i| keep[i]).or_else(|| (0..cur).rev().find(|&i| keep[i])).unwrap_or(0);
                let new_cur = keep[..next].iter().filter(|k| **k).count();
                let mut i = 0;
                side.tabs.retain(|_| {
                    i += 1;
                    keep[i - 1]
                });
                side.cur = new_cur;
            } else {
                // every tab was on the server: keep the current one and bring it home
                let mut tab = side.tabs.swap_remove(side.cur);
                side.tabs.clear();
                let home = tab.back.iter().rev().find(|p| !on_site(p) && !p.as_os_str().is_empty() && !is_remote(p)).cloned();
                if home.is_none_or(|h| tab.navigate(h).is_err()) {
                    let _ = tab.navigate(PathBuf::new());
                }
                side.tabs.push(tab);
                side.cur = 0;
            }
        }
        self.forget_site(&site);
        self.toast(trf!("Отключено: {}", "Disconnected: {}", site.url()), false);
    }

    /// Drop `site` from every tab's back/forward history and from the waiting list:
    /// its connection is closed, so those steps would only lead to an error.
    fn forget_site(&mut self, site: &Site) {
        let on_site = |p: &std::path::Path| remote::split(p).is_some_and(|(s, _)| s == *site);
        for side in &mut self.sides {
            for tab in &mut side.tabs {
                tab.back.retain(|p| !on_site(p));
                tab.forward.retain(|p| !on_site(p));
            }
        }
        self.idle_sites.retain(|s| s != site);
    }

    /// A tab showing `path` was closed. When it was the last tab on its server, the
    /// connection is closed too — right away, or once the queue is done with it.
    pub(super) fn tab_closed(&mut self, path: &std::path::Path) {
        let Some((site, _)) = remote::split(path) else { return };
        // a phone has no session to close
        if site.protocol == Protocol::Mtp {
            return;
        }
        if !self.idle_sites.contains(&site) {
            self.idle_sites.push(site);
        }
        self.release_idle_sites();
    }

    /// Close the connections of `idle_sites` that no tab shows and no queued or
    /// running operation uses. A server opened again in a tab leaves the list.
    pub(super) fn release_idle_sites(&mut self) {
        for site in std::mem::take(&mut self.idle_sites) {
            let on_site = |p: &std::path::Path| remote::split(p).is_some_and(|(s, _)| s == site);
            if self.sides.iter().any(|s| s.tabs.iter().any(|t| on_site(&t.path))) {
                continue;
            }
            if self.queue.tasks.iter().any(|t| !t.is_finished() && t.spec.paths().into_iter().any(on_site)) {
                self.idle_sites.push(site);
                continue;
            }
            if net::is_connected(&site) {
                net::disconnect(&site);
                self.forget_site(&site);
            }
        }
    }
}

/// Address-bar segments of a remote folder: `sftp://user@host`, then each folder.
pub(super) fn crumbs(path: &std::path::Path) -> Option<Vec<(String, PathBuf)>> {
    let (site, p) = remote::split(path)?;
    let mut out = vec![(site.url(), site.path("/"))];
    let mut cur = String::new();
    for seg in p.split('/').filter(|s| !s.is_empty()) {
        cur = format!("{cur}/{seg}");
        out.push((seg.to_string(), site.path(&cur)));
    }
    Some(out)
}

/// Path as the user should see and type it.
pub(super) fn shown(path: &std::path::Path) -> String {
    remote::display(path).unwrap_or_else(|| path.display().to_string())
}

/// Actions that need a local file system (Explorer, shell, clipboard, archives …).
pub(super) fn local_only(a: &Action) -> bool {
    matches!(a,
        Action::Edit | Action::ClipboardCut | Action::ClipboardCopy | Action::ClipboardPaste |
        Action::Zip | Action::Unzip | Action::FindFiles | Action::CreateWindows |
        Action::OpenInExplorer | Action::Properties | Action::FolderProperties | Action::WindowsMenu | Action::FolderWindowsMenu | Action::MultiRename | Action::ConvertImages)
}

/// Whether `path` is on an SFTP server (where an SSH terminal can be opened).
pub(super) fn is_sftp(path: &std::path::Path) -> bool {
    remote::split(path).is_some_and(|(site, _)| site.protocol == Protocol::Sftp)
}

/// Where a remote file is downloaded to be opened by a Windows program.
pub(super) fn temp_copy_dir(path: &std::path::Path) -> PathBuf {
    let host = remote::split(path).map(|(s, _)| format!("{}_{}@{}_{}", s.protocol.scheme(), s.user, s.host, s.port)).unwrap_or_default();
    let safe: String = host.chars().map(|c| if c.is_alphanumeric() || "@._-".contains(c) { c } else { '_' }).collect();
    std::env::temp_dir().join("quokka-remote").join(safe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_field_accepts_whole_addresses() {
        let mut d = ConnectDialog::new();
        d.host = "sftp://bob@example.com:2222/".into();
        assert_eq!(d.site(), Ok(Site { protocol: Protocol::Sftp, user: "bob".into(), host: "example.com".into(), port: 2222 }));
        d.host = "ftps://ann@files.example.org".into();
        assert_eq!(d.site().unwrap().url(), "ftps://ann@files.example.org");
        d.host = "example.com:2200".into();
        d.user = "alice".into();
        assert_eq!(d.site().unwrap().port, 2200);
        d.host = "::1".into();
        d.port = "22".into();
        assert_eq!(d.site().unwrap().host, "::1");
        d.user.clear();
        assert!(d.site().is_err(), "SFTP needs a user");
        d.protocol = Protocol::Ftp;
        assert_eq!(d.site().unwrap().user, "anonymous");
        d.port = "0".into();
        assert!(d.site().is_err());
    }

    #[test]
    fn switching_protocol_moves_a_default_port_only() {
        let mut d = ConnectDialog::new();
        d.set_protocol(Protocol::Ftp);
        assert_eq!(d.port, "21");
        d.port = "2121".into();
        d.set_protocol(Protocol::Sftp);
        assert_eq!(d.port, "2121");
        d.use_key = true;
        d.set_protocol(Protocol::Ftps);
        assert!(!d.use_key, "keys are SFTP only");
    }

    #[test]
    fn opening_a_server_path_uses_its_saved_connection() {
        let saved = [Connection { name: "Прод".into(), url: "sftp://bob@h:2222".into(), key_file: r"C:\k".into(), save_password: false }];
        let d = ConnectDialog::for_path(std::path::Path::new("sftp://bob@h:2222/srv/www"), &saved);
        assert_eq!((d.user.as_str(), d.host.as_str(), d.port.as_str(), d.selected), ("bob", "h", "2222", Some(0)));
        assert!(d.use_key && d.key_file == r"C:\k");
        assert!(d.auto_start, "a key without a saved passphrase is tried right away");
        assert_eq!(d.target, Some(PathBuf::from("sftp://bob@h:2222/srv/www")));
        let p = d.profile().unwrap();
        assert_eq!((p.url.as_str(), p.name.as_str()), ("sftp://bob@h:2222", "Прод"));
        let other = ConnectDialog::for_path(std::path::Path::new("ftp://x@y/pub"), &saved);
        assert_eq!((other.protocol, other.selected, other.auto_start), (Protocol::Ftp, None, false));
    }

    #[test]
    fn deleting_a_saved_connection_keeps_another_one_in_the_form() {
        let mut d = ConnectDialog::new();
        d.selected = Some(2);
        d.host = "h".into();
        d.removed(0);
        assert_eq!((d.selected, d.host.as_str()), (Some(1), "h"));
        d.removed(3);
        assert_eq!(d.selected, Some(1));
        d.removed(1);
        assert_eq!((d.selected, d.host.as_str()), (None, ""), "its own entry clears the form");
    }

    #[test]
    fn crumbs_and_display() {
        let p = std::path::Path::new("sftp://bob@h/home").join("bob");
        let c = crumbs(&p).unwrap();
        let labels: Vec<&str> = c.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["sftp://bob@h", "home", "bob"]);
        assert_eq!(c[1].1, PathBuf::from("sftp://bob@h/home"));
        assert_eq!(shown(&p), "sftp://bob@h/home/bob");
        assert!(crumbs(std::path::Path::new(r"C:\x")).is_none());
        assert!(temp_copy_dir(&p).ends_with("sftp_bob@h_22"));
    }
}
