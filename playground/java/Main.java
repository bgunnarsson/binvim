package playground;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Optional;
import java.util.stream.Collectors;

public class Main {

    public sealed interface Shape permits Circle, Square {
        double area();
        String name();
    }

    public record Circle(double radius) implements Shape {
        public double area() { return Math.PI * radius * radius; }
        public String name() { return "circle"; }
    }

    public record Square(double side) implements Shape {
        public double area() { return side * side; }
        public String name() { return "square"; }
    }

    public static <T extends Shape> Optional<T> biggest(List<T> shapes) {
        return shapes.stream().max(Comparator.comparingDouble(Shape::area));
    }

    public static String describe(Shape s) {
        return switch (s) {
            case Circle c -> "circle r=" + c.radius();
            case Square sq -> "square s=" + sq.side();
        };
    }

    public static void main(String[] args) {
        List<Shape> shapes = new ArrayList<>();
        shapes.add(new Circle(2.0));
        shapes.add(new Square(3.0));
        shapes.add(new Circle(5.0));

        List<String> labels = shapes.stream()
            .map(Main::describe)
            .collect(Collectors.toList());

        for (String label : labels) {
            System.out.println(label);
        }

        biggest(shapes).ifPresent(s -> System.out.println("biggest: " + s.name()));
    }
}
