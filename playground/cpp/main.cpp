#include <algorithm>
#include <iostream>
#include <memory>
#include <string>
#include <vector>

namespace playground {

template <typename T>
class Stack {
public:
    void push(T value) { data_.emplace_back(std::move(value)); }

    [[nodiscard]] bool empty() const noexcept { return data_.empty(); }
    [[nodiscard]] std::size_t size() const noexcept { return data_.size(); }

    T pop() {
        T v = std::move(data_.back());
        data_.pop_back();
        return v;
    }

private:
    std::vector<T> data_;
};

class Shape {
public:
    virtual ~Shape() = default;
    virtual double area() const = 0;
    virtual std::string name() const = 0;
};

class Circle final : public Shape {
public:
    explicit Circle(double r) : radius_(r) {}
    double area() const override { return 3.14159265 * radius_ * radius_; }
    std::string name() const override { return "circle"; }

private:
    double radius_;
};

class Square final : public Shape {
public:
    explicit Square(double s) : side_(s) {}
    double area() const override { return side_ * side_; }
    std::string name() const override { return "square"; }

private:
    double side_;
};

} // namespace playground

int main() {
    using namespace playground;

    std::vector<std::unique_ptr<Shape>> shapes;
    shapes.emplace_back(std::make_unique<Circle>(2.0));
    shapes.emplace_back(std::make_unique<Square>(3.0));

    std::sort(shapes.begin(), shapes.end(),
              [](const auto& a, const auto& b) { return a->area() < b->area(); });

    for (const auto& s : shapes) {
        std::cout << s->name() << " -> " << s->area() << '\n';
    }

    Stack<int> stack;
    for (int i = 0; i < 5; ++i) stack.push(i * i);
    while (!stack.empty()) std::cout << stack.pop() << ' ';
    std::cout << '\n';
}
