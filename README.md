# teravars

> [Tera] templating + smart `[vars]` handling for self-rendering TOML
> configs. Extracted from the duplicated patterns in
> [shun] / [rvpm] / [todoke] / [yui] / [spyrun].

**Status: empty. Roadmap only — see [ROADMAP.md](./ROADMAP.md).**

## What it'll do

```rust
use teravars::Engine;

let mut engine = Engine::new();          // Tera + standard helpers
let raw = std::fs::read_to_string("config.toml")?;

let vars = teravars::extract_vars(&raw)?;
let resolved = teravars::resolve(&vars, &mut engine)?;

let mut ctx = teravars::system_context();
ctx.insert_vars(&resolved);

let rendered = engine.render(&raw, &ctx)?;
let cfg: MyConfig = toml::from_str(&rendered)?;
```

The point: today, every yukimemi/* tool that consumes a TOML config
written by-hand re-implements

1. *"Pre-extract `[vars]` from the raw text so the template can
   reference its own vars."*
2. *"Iteratively resolve `vars.a = "{{ vars.b }}"` cross-refs."*
3. *"Standard Tera helpers like `env(name='X')`, `is_windows()`."*
4. *"A standard system context with os / arch / user / host."*
5. *"Multi-file merge with vars accumulating across files."*

…and they all do it slightly differently. teravars is the one place
that intent lives.

## License

Same as the parent projects: MIT.

[Tera]: https://keats.github.io/tera/
[shun]: https://github.com/yukimemi/shun
[rvpm]: https://github.com/yukimemi/rvpm
[todoke]: https://github.com/yukimemi/todoke
[yui]: https://github.com/yukimemi/yui
[spyrun]: https://github.com/yukimemi/spyrun
