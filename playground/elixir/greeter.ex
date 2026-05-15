defmodule Playground.Greeter do
  @moduledoc """
  A small Elixir module — exercises structs, guards, pattern-matching,
  pipelines, and `with` chains.
  """

  defstruct [:prefix, :exclaim?]

  @type t :: %__MODULE__{
          prefix: String.t(),
          exclaim?: boolean()
        }

  @spec new(String.t(), keyword()) :: t()
  def new(prefix \\ "Hello", opts \\ []) do
    %__MODULE__{
      prefix: prefix,
      exclaim?: Keyword.get(opts, :exclaim?, true)
    }
  end

  @spec greet(t(), String.t()) :: String.t()
  def greet(%__MODULE__{exclaim?: true} = g, name) when is_binary(name) do
    "#{g.prefix}, #{name}!"
  end

  def greet(%__MODULE__{} = g, name) when is_binary(name) do
    "#{g.prefix}, #{name}"
  end

  @spec greet_all(t(), [String.t()]) :: [String.t()]
  def greet_all(%__MODULE__{} = g, names) do
    names
    |> Enum.reject(&(&1 == ""))
    |> Enum.map(&greet(g, &1))
  end

  def lookup(map, key, default \\ nil) do
    with {:ok, value} <- Map.fetch(map, key),
         true <- value != nil do
      {:ok, value}
    else
      _ -> {:default, default}
    end
  end
end

defmodule Playground.Main do
  alias Playground.Greeter

  def run do
    g = Greeter.new("Hi")

    Greeter.greet_all(g, ["Alice", "Bob", "", "Carol"])
    |> Enum.each(&IO.puts/1)

    case Greeter.lookup(%{name: "Alice"}, :name) do
      {:ok, name} -> IO.puts("found: #{name}")
      {:default, d} -> IO.puts("missing: #{inspect(d)}")
    end
  end
end
