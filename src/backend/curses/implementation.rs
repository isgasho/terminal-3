use crate::backend::curses::mapping::find_closest;
use crate::backend::Backend;
use crate::{error, Action, Attribute, Clear, Color, Event, MouseButton, Retrieved, Value, KeyEvent, KeyModifiers, KeyCode};
use pancurses::{COLORS, SCREEN, ToChtype};
use std::collections::HashMap;
use std::io;
use std::io::Write;
use std::marker::PhantomData;
use std::sync::RwLock;
use std::fs::File;
use std::ffi::CStr;
use std::os::unix::io::IntoRawFd;

const MOUSE_EVENT_MASK: u32 = pancurses::ALL_MOUSE_EVENTS | pancurses::REPORT_MOUSE_POSITION;

pub struct BackendImpl<W: Write> {
    _phantom: PhantomData<W>,
    window: pancurses::Window,

    last_mouse_button: RwLock<Option<MouseButton>>,
    stored_event: RwLock<Option<Event>>,

    // ncurses stores color values in pairs (fg, bg) color.
    // We store those pairs in this hashmap on order to keep track of the pairs we initialized.
    color_pairs: HashMap<i16, i32>,

    screen_ptr: SCREEN,

    pub(crate) key_codes: HashMap<i32, Event>,
}

impl<W: Write> BackendImpl<W> {
    /// Prints the given string-like value into the window by printing each
 /// individual character into the window. If there is any error encountered
 /// upon printing a character, that cancels the printing of the rest of the
 /// characters.
    pub fn print<S: AsRef<str>>(&mut self, asref: S) {
        // Here we want to
        if cfg!(windows) {
            // PDCurses does an extra intermediate CString allocation, so we just
            // print out each character one at a time to avoid that.
            asref.as_ref().chars().all(|c| self.print_char(c));
        } else {
            // NCurses, it seems, doesn't do the intermediate allocation and also uses
            // a faster routine for printing a whole string at once.
            self.window.printw(asref.as_ref());
        }
    }

    /// Prints the given character into the window.
    pub fn print_char<T: ToChtype>(&mut self, character: T) -> bool {
        self.window.addch(character);

        true
    }


    pub fn update_input_buffer(&self, btn: Event) {
        let mut lock = self.stored_event.write().unwrap();
        *lock = Some(btn);
    }

    pub fn try_take(&self) -> Option<Event> {
        self.stored_event.write().unwrap().take()
    }

    pub fn update_last_btn(&self, btn: MouseButton) {
        let mut lock = self.last_mouse_button.write().unwrap();
        *lock = Some(btn);
    }

    pub fn get_last_btn(&self) -> Option<MouseButton> {
        self.last_mouse_button.read().unwrap().clone()
    }

    pub fn store_fg(&mut self, fg_color: Color) -> i32 {
        let closest_color = find_closest(fg_color, COLORS() as i16);

        if self.color_pairs.contains_key(&closest_color) {
            self.color_pairs[&closest_color]
        }else {
            let index = self.new_color_pair_index();

            self.color_pairs.insert(closest_color, index);
            pancurses::init_pair(index as i16, closest_color, -1);
            index
        }
    }


    pub fn store_bg(&mut self, bg_color: Color) -> i32 {
        let closest_color = find_closest(bg_color, COLORS() as i16);

        if self.color_pairs.contains_key(&closest_color) {
            self.color_pairs[&closest_color]
        }else {
            let index = self.new_color_pair_index();

            self.color_pairs.insert(closest_color, index);
            pancurses::init_pair(index as i16, -1, closest_color);
            index
        }
    }

    fn new_color_pair_index(&mut self) -> i32 {
        let n = 1 + self.color_pairs.len() as i32;

        if 256 > n {
            // We still have plenty of space for everyone.
            n
        } else {
            // The world is too small for both of us.
            let target = n - 1;
            // Remove the mapping to n-1
            self.color_pairs.retain(|_, &mut v| v != target);
            target
        }
    }
}

impl<W: Write> Backend<W> for BackendImpl<W> {
    fn create() -> Self {
        let file = File::open("/dev/tty").unwrap();

        let c_file = unsafe {
           libc::fdopen(
                file.into_raw_fd(),
                CStr::from_bytes_with_nul_unchecked(b"r\0").as_ptr(),
            )
        };

        let screen = unsafe { pancurses::newterm(Some(env!("TERM")), c_file, c_file) };

        let window = pancurses::stdscr();

        window.keypad(true);
        pancurses::start_color();
        pancurses::use_default_colors();
        pancurses::mousemask(pancurses::ALL_MOUSE_EVENTS | pancurses::REPORT_MOUSE_POSITION, ::std::ptr::null_mut());

        BackendImpl {
            _phantom: PhantomData,
            window,
            last_mouse_button: RwLock::new(None),
            stored_event: RwLock::new(None),
            color_pairs: HashMap::new(),
            screen_ptr: screen,
            key_codes: initialize_keymap()
        }
    }

    fn act(&mut self, action: Action, buffer: &mut W) -> error::Result<()> {
        self.batch(action, buffer)?;
        self.flush_batch(buffer)
    }

    fn batch(&mut self, action: Action, buffer: &mut W) -> error::Result<()> {
        let a = match action {
            Action::MoveCursorTo(x, y) => self.window.mv(x as i32, y as i32),
            Action::HideCursor => pancurses::curs_set(0) as i32,
            Action::ShowCursor => pancurses::curs_set(1) as i32,
            Action::EnableBlinking => pancurses::set_blink(true),
            Action::DisableBlinking => pancurses::set_blink(false),
            Action::ClearTerminal(clear_type) => {
                match clear_type {
                    Clear::All => self.window.clear(),
                    Clear::FromCursorDown => self.window.clrtobot(),
                    Clear::UntilNewLine => self.window.clrtoeol(),
                    Clear::FromCursorUp => 0, // TODO, not supported by pancurses
                    Clear::CurrentLine => 0,  // TODO, not supported by pancurses
                };

                0
            }
            Action::SetTerminalSize(cols, rows) => pancurses::resize_term(rows as i32, cols as i32),
            Action::ScrollUp(_) => 0,   // TODO, not supported by pancurses
            Action::ScrollDown(_) => 0, // TODO, not supported by pancurses
            Action::EnableRawMode => {
                pancurses::noecho();
                pancurses::raw();
                pancurses::nonl()
            }
            Action::DisableRawMode => {
                pancurses::echo();
                pancurses::noraw();
                pancurses::nl()
            }
            Action::EnterAlternateScreen => {
                self.window.mv(self.window.get_max_y() - 1, self.window.get_max_x() - 1);
                self.print("ABCDEFG");
                self.window.refresh();
                0i32
            }
            Action::LeaveAlternateScreen => {
                // TODO,
                0i32
            }
            Action::EnableMouseCapture => {
                print!("\x1B[?1002h");
                io::stdout().flush()?;
                0i32
            }
            Action::DisableMouseCapture => {
                print!("\x1B[?1002l");
                io::stdout().flush().expect("could not flush stdout");
                0i32
            }
            Action::SetForegroundColor(color) => {
                let index = self.store_fg(color);
                let style = pancurses::COLOR_PAIR(index as pancurses::chtype);
                self.window.attron(style);
                self.print("BACKGROUND");
                self.window.refresh()
            }
            Action::SetBackgroundColor(color) => {
                let index = self.store_bg(color);
                let style = pancurses::COLOR_PAIR(index as pancurses::chtype);
                self.print("FOREGROUND");
                self.window.refresh()
            }
            Action::SetAttribute(attr) => {
                let no_match1: Option<()> = match attr {
                    Attribute::Reset => Some(pancurses::Attribute::Normal),
                    Attribute::Bold => Some(pancurses::Attribute::Bold),
                    Attribute::Italic => Some(pancurses::Attribute::Italic),
                    Attribute::Underlined => Some(pancurses::Attribute::Underline),
                    Attribute::SlowBlink | Attribute::RapidBlink => {
                        Some(pancurses::Attribute::Blink)
                    }
                    Attribute::Crossed => Some(pancurses::Attribute::Overline),
                    Attribute::Reversed => Some(pancurses::Attribute::Reverse),
                    Attribute::Conceal => Some(pancurses::Attribute::Invisible),
                    _ => None, // OFF attributes and Fraktur, NormalIntensity, NormalIntensity, Framed
                }
                    .map(|attribute| {
                        self.window.attron(attribute);
                    });

                let no_match2: Option<()> = match attr {
                    Attribute::BoldOff => Some(pancurses::Attribute::Bold),
                    Attribute::ItalicOff => Some(pancurses::Attribute::Italic),
                    Attribute::UnderlinedOff => Some(pancurses::Attribute::Underline),
                    Attribute::BlinkOff => Some(pancurses::Attribute::Blink),
                    Attribute::CrossedOff => Some(pancurses::Attribute::Overline),
                    Attribute::ReversedOff => Some(pancurses::Attribute::Reverse),
                    Attribute::ConcealOff => Some(pancurses::Attribute::Invisible),
                    _ => None, // OFF attributes and Fraktur, NormalIntensity, NormalIntensity, Framed
                }
                    .map(|attribute| {
                        self.window.attroff(attribute);
                    });

                if no_match1.is_none() && no_match2.is_none() {
                    return Err(error::ErrorKind::AttributeNotSupported(String::from(attr)))?;
                } else {
                    return Ok(());
                }
            }
            Action::ResetColor => 0, // TODO
        };

        Ok(())
    }

    fn flush_batch(&mut self, buffer: &mut W) -> error::Result<()> {
        self.window.refresh();
        Ok(())
    }

    fn get(&self, retrieve_operation: Value) -> error::Result<Retrieved> {
        match retrieve_operation {
            Value::TerminalSize => {
                // Coordinates are reversed here
                let (y, x) = self.window.get_max_yx();
                Ok(Retrieved::TerminalSize(x as u16, y as u16))
            }
            Value::CursorPosition => {
                let (y, x) = self.window.get_cur_yx();
                Ok(Retrieved::CursorPosition(y as u16, x as u16))
            }
            Value::Event(duration) => {
                if let Some(event) = self.try_take() {
                    return Ok(Retrieved::Event(Some(event)));
                }

                let duration = duration.map_or(-1, |f| f.as_millis() as i32);

                self.window.timeout(duration);

                if let Some(input) = self.window.getch() {
                    return Ok(Retrieved::Event(Some(self.parse_next(input))));
                }

                Ok(Retrieved::Event(None))
            }
        }
    }
}

impl<W: Write> Drop for BackendImpl<W> {
    fn drop(&mut self) {
        print!("{}", DISABLE_MOUSE_CAPTURE);
        io::stdout().flush().expect("could not flush stdout");
        pancurses::endwin();
    }
}

fn initialize_keymap() -> HashMap<i32, Event> {
    let mut map = HashMap::default();

    fill_key_codes(&mut map, pancurses::keyname);

    map
}

fn fill_key_codes<F>(target: &mut HashMap<i32, Event>, f: F)
    where
        F: Fn(i32) -> Option<String>,
{
    let mut key_names = HashMap::<&str, KeyCode>::new();
    key_names.insert("DC", KeyCode::Delete);
    key_names.insert("DN", KeyCode::Down);
    key_names.insert("END", KeyCode::End);
    key_names.insert("HOM", KeyCode::Home);
    key_names.insert("IC", KeyCode::Insert);
    key_names.insert("LFT", KeyCode::Left);
    key_names.insert("NXT", KeyCode::PageDown);
    key_names.insert("PRV", KeyCode::PageUp);
    key_names.insert("RIT", KeyCode::Right);
    key_names.insert("UP", KeyCode::Up);

    for code in 512..1024 {
        let name = match f(code) {
            Some(name) => name,
            None => continue,
        };

        if !name.starts_with('k') {
            continue;
        }

        let (key_name, modifier) = name[1..].split_at(name.len() - 2);
        let key = match key_names.get(key_name) {
            Some(&key) => key,
            None => continue,
        };

        let event = match modifier {
            "3" => Event::Key(KeyEvent { code: key, modifiers: KeyModifiers::ALT }),
            "4" => Event::Key(KeyEvent { code: key, modifiers: KeyModifiers::ALT | KeyModifiers::SHIFT }),
            "5" => Event::Key(KeyEvent { code: key, modifiers: KeyModifiers::CONTROL }),
            "6" => Event::Key(KeyEvent { code: key, modifiers: KeyModifiers::CONTROL | KeyModifiers::CONTROL }),
            "7" => Event::Key(KeyEvent { code: key, modifiers: KeyModifiers::CONTROL | KeyModifiers::ALT }),
            _ => continue,
        };

        target.insert(code, event);
    }
}
/// A sequence of escape codes to enable terminal mouse support.
/// We use this directly instead of using `MouseTerminal` from termion.
const ENABLE_MOUSE_CAPTURE: &'static str = "\x1B[?1000h\x1b[?1002h\x1b[?1015h\x1b[?1006h";

/// A sequence of escape codes to disable terminal mouse support.
/// We use this directly instead of using `MouseTerminal` from termion.
const DISABLE_MOUSE_CAPTURE: &'static str = "\x1B[?1006l\x1b[?1015l\x1b[?1002l\x1b[?1000l";