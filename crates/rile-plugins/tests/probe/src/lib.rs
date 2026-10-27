wit_bindgen::generate!({
    path: "../../wit",
    world: "plugin",
});

use rile::plugin::host;
use rile::plugin::types::PageView;

struct Probe;

impl Guest for Probe {
    fn manifest() -> Manifest {
        Manifest {
            id: "rile.test.probe".into(),
            name: "Probe".into(),
            version: "0.0.0".into(),
            description: "sandbox probe".into(),
        }
    }

    fn commands(_ctx: Context) -> Vec<Command> {
        Vec::new()
    }

    /// `read:<path>` -> notify with the read result,
    /// `list:<path>` -> notify with the list-dir result,
    /// `spin` -> loop forever,
    /// `trap` -> panic.
    fn run_command(id: String, _ctx: Context) -> Result<Vec<Action>, String> {
        if let Some(path) = id.strip_prefix("read:") {
            return Ok(vec![Action::Notify(match host::read_text(path, 1024) {
                Ok(text) => format!("ok:{text}"),
                Err(e) => format!("err:{e}"),
            })]);
        }
        if let Some(path) = id.strip_prefix("list:") {
            return Ok(vec![Action::Notify(match host::list_dir(path) {
                Ok(names) => format!("ok:{}", names.join(",")),
                Err(e) => format!("err:{e}"),
            })]);
        }
        match id.as_str() {
            "spin" => {
                let mut x: u64 = 0;
                loop {
                    x = std::hint::black_box(x.wrapping_add(1));
                }
            }
            "trap" => panic!("probe trap"),
            "env" => Ok(vec![Action::Notify(format!(
                "env:{};fs:{}",
                std::env::vars().count(),
                std::fs::read_dir("/").is_ok()
            ))]),
            _ => Err("unknown".into()),
        }
    }

    fn preview(_path: String, _ext: String) -> Option<String> {
        None
    }

    fn pages() -> Vec<Page> {
        vec![Page { id: "probe".into(), title: "Probe".into(), icon: None }]
    }

    /// Notifies with `read-dir`/`dir-size` results for the event path
    /// (`readdir:<path>` / `size:<path>`).
    fn render_page(_req: PageRequest) -> Result<PageUpdate, String> {
        Ok(PageUpdate { view: PageView { nodes: Vec::new() }, state: String::new(), actions: Vec::new() })
    }

    fn page_event(_req: PageRequest, event: String) -> Result<PageUpdate, String> {
        let msg = if let Some(path) = event.strip_prefix("readdir:") {
            match host::read_dir(path) {
                Ok(e) => format!("ok:{}", e.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(",")),
                Err(e) => format!("err:{e}"),
            }
        } else if let Some(path) = event.strip_prefix("size:") {
            match host::dir_size(path) {
                Ok(n) => format!("ok:{n}"),
                Err(e) => format!("err:{e}"),
            }
        } else {
            return Err("unknown".into());
        };
        Ok(PageUpdate { view: PageView { nodes: Vec::new() }, state: String::new(), actions: vec![Action::Notify(msg)] })
    }
}

export!(Probe);
