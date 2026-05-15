package main

import (
	"errors"
	"fmt"
	"sort"
	"strings"
)

type Shape interface {
	Area() float64
	Name() string
}

type Circle struct {
	Radius float64
}

func (c Circle) Area() float64 { return 3.14159 * c.Radius * c.Radius }
func (c Circle) Name() string  { return "circle" }

type Rectangle struct {
	Width, Height float64
}

func (r Rectangle) Area() float64 { return r.Width * r.Height }
func (r Rectangle) Name() string  { return "rectangle" }

func biggest(shapes []Shape) (Shape, error) {
	if len(shapes) == 0 {
		return nil, errors.New("no shapes")
	}
	sort.Slice(shapes, func(i, j int) bool {
		return shapes[i].Area() > shapes[j].Area()
	})
	return shapes[0], nil
}

func main() {
	shapes := []Shape{
		Circle{Radius: 2},
		Rectangle{Width: 3, Height: 4},
		Circle{Radius: 5},
	}

	for _, s := range shapes {
		fmt.Printf("%s -> %.2f\n", strings.ToUpper(s.Name()), s.Area())
	}

	top, err := biggest(shapes)
	if err != nil {
		fmt.Println("error:", err)
		return
	}
	fmt.Println("biggest:", top.Name())

	ch := make(chan int, 3)
	go func() {
		for i := 0; i < 3; i++ {
			ch <- i * i
		}
		close(ch)
	}()
	for v := range ch {
		fmt.Println("square:", v)
	}
}
