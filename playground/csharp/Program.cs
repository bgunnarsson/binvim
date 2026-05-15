using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;

namespace Playground;

public interface IGreeter
{
    string Greet(string name);
}

public sealed class Greeter : IGreeter
{
    private readonly string _prefix;

    public Greeter(string prefix = "Hello")
    {
        _prefix = prefix ?? throw new ArgumentNullException(nameof(prefix));
    }

    public string Greet(string name) => $"{_prefix}, {name}!";
}

public record User(int Id, string Name, string? Email = null);

public static class Extensions
{
    public static IEnumerable<T> WithoutNulls<T>(this IEnumerable<T?> items) where T : class
    {
        foreach (var item in items)
        {
            if (item is not null) yield return item;
        }
    }
}

public class Program
{
    public static async Task Main(string[] args)
    {
        IGreeter greeter = new Greeter("Hi");

        var users = new List<User>
        {
            new(1, "Alice", "alice@example.com"),
            new(2, "Bob"),
            new(3, "Carol", "carol@example.com"),
        };

        var withEmail = users
            .Where(u => u.Email is not null)
            .OrderBy(u => u.Name)
            .ToList();

        foreach (var u in withEmail)
        {
            Console.WriteLine(greeter.Greet(u.Name));
        }

        await Task.Delay(10);
        Console.WriteLine($"Total: {users.Count}");
    }
}
