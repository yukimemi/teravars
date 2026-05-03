use tera::Tera;

#[cfg(feature = "std-helpers")]
mod env;
#[cfg(feature = "std-helpers")]
mod filters;
#[cfg(feature = "std-helpers")]
mod os;
#[cfg(feature = "shell")]
mod shell;

pub(crate) fn register_default(tera: &mut Tera) {
    let _ = tera;

    #[cfg(feature = "std-helpers")]
    {
        tera.register_function("env", env::env_fn);
        tera.register_function("home", os::home);
        tera.register_function("is_windows", os::is_windows);
        tera.register_function("is_linux", os::is_linux);
        tera.register_function("is_mac", os::is_mac);
        tera.register_filter("hash", filters::hash_filter);
        tera.register_filter("port_offset", filters::port_offset_filter);
    }

    #[cfg(feature = "shell")]
    shell::register(tera);
}
