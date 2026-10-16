//! viz::spec: validated chart / table specs.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Bar,
    Line,
    Pie,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSpec {
    pub kind: ChartKind,
    pub title: Option<String>,
    pub labels: Vec<String>,
    pub series: Vec<Series>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableSpec {
    pub title: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VizSpec {
    Chart(ChartSpec),
    Table(TableSpec),
}

impl VizSpec {
    pub fn title(&self) -> Option<&str> {
        match self {
            VizSpec::Chart(c) => c.title.as_deref(),
            VizSpec::Table(t) => t.title.as_deref(),
        }
    }
}

pub const MAX_LABELS: usize = 200;
pub const MAX_SERIES: usize = 8;
pub const MAX_COLUMNS: usize = 12;
pub const MAX_ROWS: usize = 200;
