const std = @import("std");

const Shape = union(enum) {
    circle: f64,
    square: f64,
    triangle: struct { a: f64, b: f64, c: f64 },

    pub fn area(self: Shape) f64 {
        return switch (self) {
            .circle => |r| std.math.pi * r * r,
            .square => |s| s * s,
            .triangle => |t| blk: {
                const s = (t.a + t.b + t.c) / 2.0;
                break :blk std.math.sqrt(s * (s - t.a) * (s - t.b) * (s - t.c));
            },
        };
    }
};

fn fib(n: u32) u32 {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    var shapes = std.ArrayList(Shape).init(allocator);
    defer shapes.deinit();

    try shapes.append(.{ .circle = 2.0 });
    try shapes.append(.{ .square = 3.0 });
    try shapes.append(.{ .triangle = .{ .a = 3.0, .b = 4.0, .c = 5.0 } });

    const stdout = std.io.getStdOut().writer();
    for (shapes.items, 0..) |s, i| {
        try stdout.print("shape #{d}: area = {d:.3}\n", .{ i, s.area() });
    }

    var sum: u32 = 0;
    for (0..10) |n| sum += fib(@intCast(n));
    try stdout.print("fib sum = {d}\n", .{sum});
}

test "fib basics" {
    try std.testing.expectEqual(@as(u32, 0), fib(0));
    try std.testing.expectEqual(@as(u32, 1), fib(1));
    try std.testing.expectEqual(@as(u32, 55), fib(10));
}
