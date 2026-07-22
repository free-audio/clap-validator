use super::WRITER;
use std::fmt::Display;

pub fn event(message: impl Display, context: impl Recordable) {
    if let Some(writer) = WRITER.get() {
        writer.lock().unwrap().write(
            format_args!("{}", message),
            std::thread::current().name().unwrap_or("?"),
            "n",
            &context,
        );
    }
}

pub struct Span {
    name: &'static str,
}

impl Drop for Span {
    fn drop(&mut self) {
        Span { name: self.name }.finish(())
    }
}

impl Span {
    pub fn begin(name: &'static str, context: impl Recordable) -> Self {
        if let Some(writer) = WRITER.get() {
            writer.lock().unwrap().write(
                format_args!("{}", name),
                std::thread::current().name().unwrap_or("?"),
                "b",
                &context,
            );
        }

        Self { name }
    }

    pub fn finish<T: Recordable>(self, context: T) {
        if let Some(writer) = WRITER.get() {
            writer.lock().unwrap().write(
                format_args!("{}", self.name),
                std::thread::current().name().unwrap_or("?"),
                "e",
                &context,
            );
        }

        std::mem::forget(self);
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

pub trait Recorder {
    fn record_value(&mut self, value: std::fmt::Arguments<'_>);
    fn record_entry(&mut self, name: &str, record: &dyn Recordable);
}

pub trait Recordable {
    fn record(&self, record: &mut dyn Recorder);
}

impl dyn Recorder + '_ {
    pub fn record<T: Recordable>(&mut self, name: &str, value: T) {
        self.record_entry(name, &value);
    }
}

impl Recordable for () {
    fn record(&self, _: &mut dyn Recorder) {}
}

impl<T: Recordable + ?Sized> Recordable for &T {
    fn record(&self, record: &mut dyn Recorder) {
        (*self).record(record);
    }
}

impl<T: Recordable> Recordable for Option<T> {
    fn record(&self, record: &mut dyn Recorder) {
        if let Some(value) = self {
            value.record(record);
        }
    }
}

macro_rules! impl_display {
    ($($ty:ty),*) => {
        $(impl Recordable for $ty {
            fn record(&self, record: &mut dyn Recorder) {
                record.record_value(format_args!("{}", self));
            }
        })*
    };
}

impl_display!(
    bool,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64,
    str,
    String,
    std::borrow::Cow<'_, str>,
    std::fmt::Arguments<'_>
);

pub fn from_fn(f: impl Fn(&mut dyn Recorder)) -> impl Recordable {
    struct FnRecord<F: Fn(&mut dyn Recorder)>(F);
    impl<F: Fn(&mut dyn Recorder)> Recordable for FnRecord<F> {
        fn record(&self, record: &mut dyn Recorder) {
            (self.0)(record);
        }
    }
    FnRecord(f)
}

macro_rules! record {
    ($($name:ident: $value:expr),*) => {{
        $crate::cli::tracing::from_fn(|record| {
            $(record.record_entry(stringify!($name), &$value as &dyn $crate::cli::tracing::Recordable);)*
        })
    }};
}

pub(crate) use record;
