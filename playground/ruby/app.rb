#!/usr/bin/env ruby
# frozen_string_literal: true

# Sample Ruby script — classes, modules, blocks, symbols, strings.

module Greetable
  def greet(name)
    "#{salutation}, #{name}!"
  end
end

class User
  include Greetable

  attr_reader :id, :name
  attr_accessor :email

  def initialize(id:, name:, email: nil)
    @id = id
    @name = name
    @email = email
  end

  def salutation
    "Hello"
  end

  def to_s
    "#<User id=#{@id} name=#{@name.inspect}>"
  end
end

class Admin < User
  def salutation
    "Welcome back"
  end
end

users = [
  User.new(id: 1, name: "Alice", email: "alice@example.com"),
  Admin.new(id: 2, name: "Bob"),
  User.new(id: 3, name: "Carol")
]

users
  .select { |u| !u.email.nil? }
  .sort_by(&:name)
  .each do |u|
    puts u.greet(u.name)
  end

squares = (1..5).map { |n| n * n }
puts "squares: #{squares.join(', ')}"

regex = /^(?<scheme>https?):\/\/(?<host>[^\/]+)/
if (m = "https://example.com/path".match(regex))
  puts "scheme=#{m[:scheme]} host=#{m[:host]}"
end
