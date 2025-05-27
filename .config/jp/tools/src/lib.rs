mod cargo;
mod github;
pub mod server;

#[derive(Default)]
#[expect(dead_code)]
pub struct ToolsServer(std::sync::Mutex<Data>);

#[derive(Default)]
struct Data {
    _todo: (),
}

fn to_xml<T: serde::Serialize>(failures: T) -> String {
    let mut buffer = String::new();
    let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
    serializer.indent(' ', 2);
    match failures.serialize(serializer) {
        Ok(_) => buffer,
        Err(error) => format!("<error>{error}</error>"),
    }
}
