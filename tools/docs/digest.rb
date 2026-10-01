# frozen_string_literal: true
# Prints a compact digest of the examples (code / data / output) found in the downloaded
# Shopify reference pages. Usage: ruby tools/docs/digest.rb filters money image_url ...
require "json"
Encoding.default_external = Encoding::UTF_8

def examples(markdown)
  out = []
  current = nil
  section = nil
  buffer = nil
  markdown.each_line do |line|
    if line =~ /\A#####\s+(Code|Data|Output)/
      section = Regexp.last_match(1).downcase
      current = {} if section == "code"
      next
    end
    if line.start_with?("```")
      if buffer
        current[section] = buffer.join if current && section
        out << current if section == "output" && current
        buffer = nil
        section = nil if section == "output"
      elsif section
        buffer = []
      end
      next
    end
    buffer << line if buffer
  end
  out
end

kind = ARGV.shift
names = ARGV.empty? ? Dir.children(".cache/shopify-docs/#{kind}").map { |f| f.delete_suffix(".md") }.sort : ARGV
limit = (ENV["LIMIT"] || 700).to_i
names.each do |name|
  path = ".cache/shopify-docs/#{kind}/#{name}.md"
  next unless File.exist?(path)
  puts "=== #{name}"
  examples(File.read(path)).each do |ex|
    puts "  CODE: #{ex["code"].to_s.strip[0, limit]}"
    puts "  DATA: #{ex["data"].to_s.gsub(/\s+/, " ").strip[0, limit]}" if ex["data"] && ENV["DATA"]
    puts "  OUT:  #{ex["output"].to_s.strip[0, limit]}"
    puts
  end
end
