use std::path::PathBuf;

use crate::settings::Settings;

#[derive(Clone, Debug)]
pub struct AppSettings {
    pub auto_device: bool,
    pub device_filter: String,
    pub device: String,

    pub auto_update: bool,
    pub frequency: f64,
    pub samplerate: f64,
    pub gain: f64,
    pub automatic_gain: bool,
    pub automatic_dc_offset: bool,
}

impl AppSettings {
    pub fn pretty_serialize(&self) -> String {
        let AppSettings {
            auto_device: auto_select_device,
            device_filter,
            device,
            auto_update,
            frequency,
            samplerate,
            gain,
            automatic_gain,
            automatic_dc_offset,
            ..
        } = self.clone();

        format!(
            r#"auto_device = {:8}      # if true, the application tries to immediatelly select a device without user input
device_filter = {:8}    # the "args" used to filter the SoapySDR devices, for example 'driver=RTLSDR' or 'hardware=R820T' 
device = {:8}           # the 'label' field of the device used last time, auto_select_device first tries to find a device with this label

auto_update = {:8}      # whether to update the receiver configuration immediatelly after a value is changed

    # values of the different configuration options
    frequency = {} # MHz
    samplerate = {} # MSps
    gain = {} # dB
    automatic_gain = "{}"
    automatic_dc_offset = "{}""#,
            // the data is first formatted into a string before being interpolated into the main string
            // so that the minimum width-format is correct
            format!("\"{}\"", auto_select_device),
            format!("\"{}\"", device_filter),
            format!("\"{}\"", device),
            format!("\"{}\"", auto_update),
            frequency,
            samplerate,
            gain,
            automatic_gain,
            automatic_dc_offset,
        )
    }
}

pub const DEFAULT_SETTINGS: AppSettings = AppSettings {
    auto_device: false,
    device_filter: String::new(),
    device: String::new(),

    auto_update: false,
    frequency: 0.0,
    samplerate: 0.0,
    gain: 0.0,
    automatic_gain: false,
    automatic_dc_offset: false,
};

//                      (Settings, Save path)
pub fn get_settings() -> (AppSettings, Option<PathBuf>) {
    const HELP: &str = "\
Overview: Tool for receiving transmission from weather baloons.

Usage: radiothing [options]

Options:
--create-config       Write default config file to provided path and immediatelly exit, CWD if empty.
-c, --config          Path to configuration file and/or the path the config will be saved to,
                      by default the current working directory.
-i, --ignore-config   Ignore any configuration file, don't save upon exit either.
-s, --save-config     Path to save the configuration on program exit, by default same as path.
-h, --help            Print this help.
";

    // we want to exit on any errors here, also it seems that optional values don't play well with the parsing mechanism
    fn handle_error(result: Result<Option<PathBuf>, pico_args::Error>) -> Option<PathBuf> {
        match result {
            Ok(ok) => ok,
            // this error is emitted if there is no argument and the option is last in the invocation
            // ie 'radioting -c' triggers it, this isn't really an error if the value is optional
            // however the parsing function 'find_value' isn't given that information
            Err(pico_args::Error::OptionWithoutAValue(_)) => None,
            Err(e) => {
                log::error!("Error parsing args: {}", e);
                std::process::exit(1);
            }
        }
    }

    let mut args = pico_args::Arguments::from_env();

    if args.contains(["-h", "--help"]) {
        print!("{}", HELP);
        std::process::exit(0);
    }

    if args.contains("--create-config") {
        // for some reason here the OptionWithoutAValue error isn't emitted? why?
        let path = handle_error(args.opt_value_from_str("--create-config")).unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap()
                .join("radiothing_config.txt")
        });

        log::info!(
            "Creating default configuration at '{}'",
            path.to_string_lossy()
        );

        match std::fs::write(&path, DEFAULT_SETTINGS.pretty_serialize()) {
            Err(e) => log::error!(
                "Error writing config to file at '{}': {}",
                path.to_string_lossy(),
                e
            ),
            Ok(_) => {}
        }

        std::process::exit(0);
    }

    if !args.contains(["-i", "--ignore-config"]) {
        let path = handle_error(args.opt_value_from_str(["-c", "--config"])).unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap()
                .join("radiothing_config.txt")
        });

        let save_path =
            if let Some(save) = handle_error(args.opt_value_from_str(["-s", "--save-config"])) {
                Some(save)
            } else if args.contains(["-s", "--save-config"]) {
                Some(path.clone())
            } else {
                None
            };

        log::info!("Reading configuration file at '{}'", path.to_string_lossy());

        match &save_path {
            Some(path) => log::info!("Save path is '{}'", path.to_string_lossy()),
            None => log::info!("Will not save config"),
        }

        if path.is_file() {
            let string = match std::fs::read_to_string(&path) {
                Ok(string) => string,
                Err(e) => {
                    log::error!(
                        "Error reading config file at '{}': {}",
                        path.to_string_lossy(),
                        e
                    );
                    return (DEFAULT_SETTINGS, save_path);
                }
            };

            let (settings, errors) = Settings::new(string.as_str());

            if !errors.is_empty() {
                let errors_string: String = errors.iter().map(|e| e.to_string() + "\n").collect();
                log::error!(
                    "Encountered errors while parsing settings, falling back to defaults:\n{}",
                    errors_string
                );

                // the parsed settings can't be assumed to be correct
                // fall back to the defaults but set save_config to false so that
                // we don't overwrite the bad settings file in case the error there is only minor
                return (DEFAULT_SETTINGS, None);
            } else {
                macro_rules! settings_from_settings {
                        ($($field:ident),* $(,)*) => {
                            AppSettings {
                                $(
                                    $field: settings.get(stringify!($field)).unwrap(),
                                )*
                            }
                        }
                    }

                let settings = settings_from_settings! {
                    auto_device,
                    device,
                    device_filter,
                    auto_update,
                    frequency,
                    samplerate,
                    gain,
                    automatic_gain,
                    automatic_dc_offset,
                };

                return (settings, save_path);
            }
        } else {
            log::error!("Config file at '{}' is not a file", path.to_string_lossy())
        }
    } else {
        log::info!("Ignoring config");
    }

    return (DEFAULT_SETTINGS, None);
}
