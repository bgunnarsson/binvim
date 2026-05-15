use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}

impl<T: fmt::Display + Copy> Point<T> {
    pub fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    pub fn coords(&self) -> (T, T) {
        (self.x, self.y)
    }
}

pub trait Greet {
    fn greet(&self) -> String;
}

pub enum Shape {
    Circle(f64),
    Square { side: f64 },
    Triangle(f64, f64, f64),
}

impl Shape {
    pub fn area(&self) -> f64 {
        match self {
            Shape::Circle(r) => std::f64::consts::PI * r * r,
            Shape::Square { side } => side * side,
            Shape::Triangle(a, b, c) => {
                let s = (a + b + c) / 2.0;
                (s * (s - a) * (s - b) * (s - c)).sqrt()
            }
        }
    }
}

async fn fetch_data(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(format!("data from {}", url))
}

fn main() {
    let p = Point::new(3, 4);
    println!("point = {:?}, coords = {:?}", p, p.coords());

    let shapes = vec![
        Shape::Circle(1.5),
        Shape::Square { side: 2.0 },
        Shape::Triangle(3.0, 4.0, 5.0),
    ];

    let mut totals: HashMap<&str, f64> = HashMap::new();
    for (i, s) in shapes.iter().enumerate() {
        let key = match s {
            Shape::Circle(_) => "circle",
            Shape::Square { .. } => "square",
            Shape::Triangle(..) => "triangle",
        };
        *totals.entry(key).or_insert(0.0) += s.area();
        println!("shape #{}: area = {:.3}", i, s.area());
    }

    let _ = (0..5).filter(|n| n % 2 == 0).sum::<i32>();
}
