#!/usr/bin/env ruby
# frozen_string_literal: true

# -------------------------------------------------------------------------------------------------
#  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
#  https://nautechsystems.io
#
#  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
#  You may not use this file except in compliance with the License.
#  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
#
#  Unless required by applicable law or agreed to in writing, software
#  distributed under the License is distributed on an "AS IS" BASIS,
#  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
#  See the License for the specific language governing permissions and
#  limitations under the License.
# -------------------------------------------------------------------------------------------------

require "date"
require "digest"
require "fileutils"
require "json"
require "net/http"
require "optparse"
require "pathname"
require "set"
require "tempfile"
require "time"
require "uri"
require "yaml"

SOURCE_URL = "https://api.unusualwhales.com/api/openapi"
HTTP_METHODS = %w[get put post delete options head patch trace].freeze
SUPPORTED_HTTP_METHODS = %w[get post].freeze
TOP_LEVEL_KEYS = %w[components info openapi paths security servers tags].freeze
COMPONENT_KEYS = %w[responses schemas securitySchemes].freeze
OPERATION_KEYS = %w[
  callbacks description operationId parameters requestBody responses summary tags
].freeze
PARAMETER_KEYS = %w[
  allowReserved deprecated description example examples explode in name required schema style
].freeze
REQUEST_BODY_KEYS = %w[content description required].freeze
RESPONSE_KEYS = %w[content description headers links].freeze
MEDIA_TYPE_KEYS = %w[encoding example examples schema].freeze
SCHEMA_KEYS = %w[
  $ref additionalProperties allOf anyOf default deprecated description discriminator enum example
  exclusiveMaximum exclusiveMinimum format items maximum maxItems maxLength maxProperties minimum
  minItems minLength minProperties multipleOf not nullable oneOf pattern prefixItems properties
  readOnly required title type uniqueItems writeOnly
].freeze
SCHEMA_TYPES = [nil, "array", "boolean", "date", "integer", "null", "number", "object", "string"].freeze
PARAMETER_LOCATIONS = %w[path query header cookie].freeze

Options = Struct.new(:check, :fetch, :source, keyword_init: true)

def parse_options
  options = Options.new(check: false, fetch: false, source: nil)
  OptionParser.new do |parser|
    parser.banner = "Usage: generate_contract.rb [--fetch | --source PATH] [--check]"
    parser.on("--check", "Fail when committed artifacts differ") { options.check = true }
    parser.on("--fetch", "Fetch the official OpenAPI source") { options.fetch = true }
    parser.on("--source PATH", "Read an existing OpenAPI YAML source") { |path| options.source = path }
  end.parse!

  abort("Choose only one of --fetch or --source") if options.fetch && options.source
  options.fetch = true unless options.source
  options
end

def fetch_source
  uri = URI(SOURCE_URL)
  response = Net::HTTP.get_response(uri)
  abort("OpenAPI fetch failed with HTTP #{response.code}") unless response.is_a?(Net::HTTPSuccess)

  response.body
end

def parse_yaml(source)
  YAML.safe_load(
    source,
    permitted_classes: [Date, Time],
    permitted_symbols: [],
    aliases: true,
  )
rescue Psych::Exception => e
  abort("OpenAPI YAML is invalid: #{e.message}")
end

def canonical(value)
  case value
  when Hash
    value.keys.sort.each_with_object({}) { |key, result| result[key] = canonical(value.fetch(key)) }
  when Array
    value.map { |item| canonical(item) }
  when Date, Time
    value.iso8601
  else
    value
  end
end

def canonical_json(value)
  JSON.generate(canonical(value))
end

def assert_exact_keys!(value, allowed, context)
  unknown = value.keys - allowed
  abort("#{context} has unclassified keys: #{unknown.sort.join(', ')}") unless unknown.empty?
end

def resolve_ref(document, reference)
  abort("Only local OpenAPI references are supported: #{reference}") unless reference.start_with?("#/")

  schema_prefix = "#/components/schemas/"
  if reference.start_with?(schema_prefix)
    schema_name = reference.delete_prefix(schema_prefix)
    schemas = document.dig("components", "schemas")
    return schemas.fetch(schema_name) if schemas.is_a?(Hash) && schemas.key?(schema_name)
    trimmed_name = schema_name.strip
    return schemas.fetch(trimmed_name) if schemas.is_a?(Hash) && schemas.key?(trimmed_name)
  end

  reference.delete_prefix("#/").split("/").reduce(document) do |value, token|
    key = token.gsub("~1", "/").gsub("~0", "~")
    abort("Unresolved OpenAPI reference: #{reference}") unless value.is_a?(Hash) && value.key?(key)

    value.fetch(key)
  end
end

def validate_schema!(schema, document, context, seen = Set.new)
  abort("#{context} schema must be an object") unless schema.is_a?(Hash)
  assert_exact_keys!(schema, SCHEMA_KEYS, "#{context} schema")

  type = schema["type"]
  abort("#{context} has unclassified schema type: #{type.inspect}") unless SCHEMA_TYPES.include?(type)
  abort("#{context} enum must be an array") if schema.key?("enum") && !schema["enum"].is_a?(Array)
  abort("#{context} required must be an array") if schema.key?("required") && !schema["required"].is_a?(Array)
  if schema.key?("$ref")
    reference = schema.fetch("$ref")
    abort("#{context} $ref must be a string") unless reference.is_a?(String)
    unless seen.include?(reference)
      validate_schema!(resolve_ref(document, reference), document, reference, seen | [reference])
    end
  end

  validate_schema!(schema.fetch("items"), document, "#{context}.items", seen) if schema["items"].is_a?(Hash)
  validate_schema!(schema.fetch("not"), document, "#{context}.not", seen) if schema["not"].is_a?(Hash)

  %w[allOf anyOf oneOf prefixItems].each do |composition|
    next unless schema.key?(composition)

    values = schema.fetch(composition)
    abort("#{context}.#{composition} must be an array") unless values.is_a?(Array)
    values.each_with_index do |child, index|
      validate_schema!(child, document, "#{context}.#{composition}[#{index}]", seen)
    end
  end

  if schema.key?("properties")
    properties = schema.fetch("properties")
    abort("#{context}.properties must be an object") unless properties.is_a?(Hash)
    properties.each do |name, child|
      validate_schema!(child, document, "#{context}.properties.#{name}", seen)
    end
  end

  additional = schema["additionalProperties"]
  unless additional.nil? || additional == true || additional == false
    validate_schema!(additional, document, "#{context}.additionalProperties", seen)
  end
end

def validate_content!(content, document, context)
  abort("#{context} content must be an object") unless content.is_a?(Hash)
  content.each do |media_type, media|
    abort("#{context} media type must be a string") unless media_type.is_a?(String)
    abort("#{context}.#{media_type} must be an object") unless media.is_a?(Hash)
    assert_exact_keys!(media, MEDIA_TYPE_KEYS, "#{context}.#{media_type}")
    abort("#{context}.#{media_type} has unclassified encoding") if media.key?("encoding")
    validate_schema!(media.fetch("schema"), document, "#{context}.#{media_type}") if media.key?("schema")
  end
end

def validate_parameter!(parameter, document, context)
  abort("#{context} parameter must be an object") unless parameter.is_a?(Hash)
  assert_exact_keys!(parameter, PARAMETER_KEYS, context)

  location = parameter.fetch("in") { abort("#{context} is missing in") }
  abort("#{context} has unclassified location: #{location.inspect}") unless PARAMETER_LOCATIONS.include?(location)
  name = parameter.fetch("name") { abort("#{context} is missing name") }
  abort("#{context} name must be non-empty") unless name.is_a?(String) && !name.empty?
  abort("#{context} path parameter must be required") if location == "path" && parameter["required"] != true
  abort("#{context} required must be boolean") if parameter.key?("required") && ![true, false].include?(parameter["required"])
  abort("#{context} explode must be boolean") if parameter.key?("explode") && ![true, false].include?(parameter["explode"])
  abort("#{context} allowReserved must be boolean") if parameter.key?("allowReserved") && ![true, false].include?(parameter["allowReserved"])

  style = effective_style(parameter)
  valid_style = (location == "query" && style == "form") || (location == "path" && style == "simple")
  abort("#{context} has unclassified serialization style: #{style}") unless valid_style

  schema = parameter.fetch("schema") { abort("#{context} is missing schema") }
  validate_schema!(schema, document, context)
end

def validate_response!(response, document, context)
  abort("#{context} response must be an object") unless response.is_a?(Hash)
  assert_exact_keys!(response, RESPONSE_KEYS, context)
  abort("#{context} has unclassified headers") if response.key?("headers")
  abort("#{context} has unclassified links") if response.key?("links")
  validate_content!(response.fetch("content"), document, context) if response.key?("content")
end

def validate_document_metadata!(document)
  abort("Only OpenAPI 3.0.0 is classified") unless document["openapi"] == "3.0.0"
  info = document.fetch("info")
  abort("OpenAPI info must be an object") unless info.is_a?(Hash)
  assert_exact_keys!(info, %w[description title version], "OpenAPI info")

  servers = document.fetch("servers")
  abort("OpenAPI servers must be an array") unless servers.is_a?(Array)
  servers.each_with_index do |server, index|
    abort("OpenAPI server #{index} must be an object") unless server.is_a?(Hash)
    assert_exact_keys!(server, %w[description url variables], "OpenAPI server #{index}")
    variables = server.fetch("variables", {})
    abort("OpenAPI server #{index} variables must be an object") unless variables.is_a?(Hash)
    abort("OpenAPI server variables are not classified") unless variables.empty?
  end

  tags = document.fetch("tags")
  abort("OpenAPI tags must be an array") unless tags.is_a?(Array)
  abort("OpenAPI tag objects are not classified") unless tags.empty?

  security = document.fetch("security")
  abort("OpenAPI security must be an array") unless security.is_a?(Array)
  security.each do |requirement|
    abort("OpenAPI security requirement must be an object") unless requirement.is_a?(Hash)
    requirement.each_value do |scopes|
      abort("OpenAPI security scopes must be an array") unless scopes.is_a?(Array)
    end
  end

  schemes = document.fetch("components").fetch("securitySchemes")
  abort("OpenAPI security schemes must be an object") unless schemes.is_a?(Hash)
  schemes.each do |name, scheme|
    abort("Security scheme #{name} must be an object") unless scheme.is_a?(Hash)
    assert_exact_keys!(
      scheme,
      %w[bearerFormat description in name openIdConnectUrl scheme type],
      "Security scheme #{name}",
    )
    abort("Only HTTP bearer security schemes are classified") unless
      scheme["type"] == "http" && scheme["scheme"] == "bearer"
  end
end

def response_refs(value, refs = Set.new)
  case value
  when Hash
    refs << value.fetch("$ref") if value["$ref"].is_a?(String)
    value.each_value { |child| response_refs(child, refs) }
  when Array
    value.each { |child| response_refs(child, refs) }
  end
  refs
end

def resolved_schema(schema, document, seen = Set.new)
  return canonical(schema) unless schema.is_a?(Hash)

  if schema["$ref"].is_a?(String)
    reference = schema.fetch("$ref")
    abort("Recursive parameter schema reference: #{reference}") if seen.include?(reference)

    resolved = resolved_schema(resolve_ref(document, reference), document, seen | [reference])
    siblings = schema.reject { |key, _| key == "$ref" }
    return canonical(resolved.merge(siblings))
  end

  schema.each_with_object({}) do |(key, value), result|
    result[key] = case key
                  when "items", "not", "additionalProperties"
                    value.is_a?(Hash) ? resolved_schema(value, document, seen) : canonical(value)
                  when "allOf", "anyOf", "oneOf", "prefixItems"
                    Array(value).map { |child| resolved_schema(child, document, seen) }
                  when "properties"
                    value.transform_values { |child| resolved_schema(child, document, seen) }
                  else
                    canonical(value)
                  end
  end
end

def effective_style(parameter)
  parameter["style"] || (parameter.fetch("in") == "query" ? "form" : "simple")
end

def effective_explode(parameter)
  return parameter.fetch("explode") if parameter.key?("explode")

  effective_style(parameter) == "form"
end

def collect_operations(document)
  paths = document.fetch("paths")
  abort("OpenAPI paths must be an object") unless paths.is_a?(Hash)
  operations = []

  paths.each do |path, path_item|
    abort("Path item #{path} must be an object") unless path_item.is_a?(Hash)
    unknown = path_item.keys - HTTP_METHODS
    abort("Path #{path} has unclassified keys: #{unknown.sort.join(', ')}") unless unknown.empty?

    path_item.each do |method, operation|
      abort("Unsupported HTTP method #{method.upcase} for #{path}") unless SUPPORTED_HTTP_METHODS.include?(method)
      abort("Operation #{method.upcase} #{path} must be an object") unless operation.is_a?(Hash)
      assert_exact_keys!(operation, OPERATION_KEYS, "Operation #{method.upcase} #{path}")
      abort("Operation #{method.upcase} #{path} has non-empty callbacks") unless operation.fetch("callbacks", {}).empty?

      operation_id = operation.fetch("operationId") { abort("Operation #{method.upcase} #{path} has no operationId") }
      parameters = operation.fetch("parameters", [])
      abort("#{operation_id} parameters must be an array") unless parameters.is_a?(Array)
      parameters.each_with_index do |parameter, index|
        validate_parameter!(parameter, document, "#{operation_id}.parameters[#{index}]")
      end

      placeholders = path.scan(/\{([^}]+)\}/).flatten.sort
      path_parameters = parameters.select { |parameter| parameter["in"] == "path" }.map { |parameter| parameter["name"] }.sort
      abort("#{operation_id} path placeholders do not match parameters") unless placeholders == path_parameters

      if operation.key?("requestBody")
        body = operation.fetch("requestBody")
        abort("#{operation_id} requestBody must be an object") unless body.is_a?(Hash)
        assert_exact_keys!(body, REQUEST_BODY_KEYS, "#{operation_id}.requestBody")
        validate_content!(body.fetch("content"), document, "#{operation_id}.requestBody")
      end

      responses = operation.fetch("responses") { abort("#{operation_id} has no responses") }
      abort("#{operation_id} responses must be an object") unless responses.is_a?(Hash)
      responses.each { |status, response| validate_response!(response, document, "#{operation_id}.responses.#{status}") }

      operations << {
        "operation_id" => operation_id,
        "method" => method.upcase,
        "path" => path,
        "classification" => method == "get" ? "read" : "account_mutation",
        "parameters" => parameters.map do |parameter|
          {
            "name" => parameter.fetch("name"),
            "in" => parameter.fetch("in"),
            "required" => parameter.fetch("required", false),
            "style" => effective_style(parameter),
            "explode" => effective_explode(parameter),
            "allow_reserved" => parameter.fetch("allowReserved", false),
            "schema" => canonical(parameter.fetch("schema")),
            "resolved_schema" => resolved_schema(parameter.fetch("schema"), document),
          }
        end,
        "request_body" => operation.key?("requestBody") ? canonical(operation.fetch("requestBody")) : nil,
        "responses" => canonical(responses),
        "response_schema_refs" => response_refs(responses).to_a.sort,
        "security" => canonical(operation.fetch("security", document.fetch("security", []))),
      }
    end
  end

  duplicates = operations.group_by { |operation| operation.fetch("operation_id") }.select { |_, values| values.length > 1 }
  abort("Duplicate operation IDs: #{duplicates.keys.sort.join(', ')}") unless duplicates.empty?
  operations.sort_by { |operation| operation.fetch("operation_id") }
end

def collect_channels(operations)
  operation = operations.find do |candidate|
    candidate.fetch("operation_id") == "PublicApi.SocketController.channels"
  end
  abort("WebSocket channel catalog operation is missing") unless operation

  # Use the original operation description because the normalized operation catalog intentionally
  # omits prose except for the channel rows needed at runtime.
  operation
end

def extract_channels(document)
  description = document.fetch("paths").fetch("/api/socket").fetch("get").fetch("description")
  lines = description.lines.map(&:strip)
  start = lines.index("The following channels are available:")
  abort("WebSocket channel table marker is missing") unless start

  rows = []
  lines[(start + 1)..].each do |line|
    next if line.empty? && rows.empty?
    next if line.match?(/^\|[-| ]+\|$/)
    break if line.empty? && !rows.empty?
    next unless line.start_with?("|")

    cells = line.split("|", -1)[1...-1].map(&:strip)
    next if cells[0] == "Channel"
    abort("Unclassified WebSocket channel table row: #{line}") unless cells.length == 2
    rows << { "form" => cells[0], "description" => cells[1] }
  end

  abort("WebSocket channel table is empty") if rows.empty?
  duplicates = rows.group_by { |row| row.fetch("form") }.select { |_, values| values.length > 1 }
  abort("Duplicate WebSocket channel forms: #{duplicates.keys.sort.join(', ')}") unless duplicates.empty?

  rows.each do |row|
    form = row.fetch("form")
    abort("Invalid WebSocket channel form: #{form}") unless form.match?(/\A[a-z][a-z0-9_-]*(?::TICKER)?\z/)
  end
  rows
end

def rust_string(value)
  value.to_s.dump.gsub(/\\u([0-9a-fA-F]{4})/, '\\u{\1}')
end

def operation_variant(operation_id)
  name = operation_id.delete_prefix("PublicApi.").split(/[^A-Za-z0-9]+/).map do |part|
    part.split("_").map { |word| word[0].upcase + word[1..] }.join
  end.join
  name = "Operation#{name}" if name.match?(/\A\d/)
  name
end

def channel_variant(form)
  base = form.delete_suffix(":TICKER").split(/[^A-Za-z0-9]+/).map(&:capitalize).join
  "#{base}#{form.end_with?(':TICKER') ? 'Ticker' : 'All'}"
end

def render_rust(metadata, operations, channels)
  operation_variants = operations.to_h { |operation| [operation_variant(operation.fetch("operation_id")), operation] }
  abort("Generated operation enum variants collide") unless operation_variants.length == operations.length
  channel_variants = channels.to_h { |channel| [channel_variant(channel.fetch("form")), channel] }
  abort("Generated channel enum variants collide") unless channel_variants.length == channels.length

  lines = []
  lines << "// -------------------------------------------------------------------------------------------------"
  lines << "//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved."
  lines << "//  https://nautechsystems.io"
  lines << "//"
  lines << "//  Licensed under the GNU Lesser General Public License Version 3.0 (the \"License\");"
  lines << "//  You may not use this file except in compliance with the License."
  lines << "//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html"
  lines << "//"
  lines << "//  Unless required by applicable law or agreed to in writing, software"
  lines << "//  distributed under the License is distributed on an \"AS IS\" BASIS,"
  lines << "//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied."
  lines << "//  See the License for the specific language governing permissions and"
  lines << "//  limitations under the License."
  lines << "// -------------------------------------------------------------------------------------------------"
  lines << ""
  lines << "//! Generated Unusual Whales REST and WebSocket contract."
  lines << "//!"
  lines << "//! Generated by `scripts/generate_contract.rb`. Do not edit by hand."
  lines << ""
  lines << "pub const SOURCE_URL: &str = #{rust_string(metadata.fetch('source_url'))};"
  lines << "pub const SOURCE_SHA256: &str = #{rust_string(metadata.fetch('source_sha256'))};"
  lines << "pub const PATH_COUNT: usize = #{metadata.fetch('path_count')};"
  lines << "pub const OPERATION_COUNT: usize = #{metadata.fetch('operation_count')};"
  lines << "pub const GET_OPERATION_COUNT: usize = #{metadata.fetch('method_counts').fetch('GET')};"
  lines << "pub const POST_OPERATION_COUNT: usize = #{metadata.fetch('method_counts').fetch('POST')};"
  lines << "pub const CHANNEL_FORM_COUNT: usize = #{metadata.fetch('channel_form_count')};"
  lines << ""
  lines << "#[derive(Clone, Copy, Debug, Eq, PartialEq)]"
  lines << "pub enum OperationClassification {"
  lines << "    Read,"
  lines << "    AccountMutation,"
  lines << "}"
  lines << ""
  lines << "#[derive(Clone, Copy, Debug, Eq, PartialEq)]"
  lines << "pub struct ParameterSpec {"
  lines << "    pub name: &'static str,"
  lines << "    pub location: &'static str,"
  lines << "    pub required: bool,"
  lines << "    pub style: &'static str,"
  lines << "    pub explode: bool,"
  lines << "    pub allow_reserved: bool,"
  lines << "    pub schema_json: &'static str,"
  lines << "    pub resolved_schema_json: &'static str,"
  lines << "}"
  lines << ""
  lines << "#[derive(Clone, Copy, Debug, Eq, PartialEq)]"
  lines << "pub struct OperationSpec {"
  lines << "    pub operation_id: &'static str,"
  lines << "    pub method: &'static str,"
  lines << "    pub path: &'static str,"
  lines << "    pub classification: OperationClassification,"
  lines << "    pub parameters: &'static [ParameterSpec],"
  lines << "    pub request_body_json: Option<&'static str>,"
  lines << "    pub responses_json: &'static str,"
  lines << "    pub response_schema_refs: &'static [&'static str],"
  lines << "    pub security_json: &'static str,"
  lines << "}"
  lines << ""
  lines << "#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]"
  lines << "#[repr(u16)]"
  lines << "#[cfg_attr("
  lines << "    feature = \"python\","
  lines << "    pyo3::pyclass(module = \"nautilus_trader.adapters.unusual_whales\", eq, from_py_object)"
  lines << ")]"
  lines << "#[cfg_attr("
  lines << "    feature = \"python\","
  lines << "    pyo3_stub_gen::derive::gen_stub_pyclass_enum("
  lines << "        module = \"nautilus_trader.adapters.unusual_whales\""
  lines << "    )"
  lines << ")]"
  lines << "pub enum UnusualWhalesOperationId {"
  operation_variants.each_key { |variant| lines << "    #{variant}," }
  lines << "}"
  lines << ""
  lines << "pub static OPERATION_IDS: &[&str] = &["
  operation_variants.each_value do |operation|
    lines << "    #{rust_string(operation.fetch('operation_id'))},"
  end
  lines << "];"
  lines << ""
  lines << "impl UnusualWhalesOperationId {"
  lines << "    #[must_use]"
  lines << "    pub fn operation_id(self) -> &'static str {"
  lines << "        OPERATION_IDS[self as usize]"
  lines << "    }"
  lines << "}"
  lines << ""
  lines << "#[derive(Clone, Copy, Debug, Eq, PartialEq)]"
  lines << "pub struct ChannelSpec {"
  lines << "    pub form: &'static str,"
  lines << "    pub prefix: &'static str,"
  lines << "    pub requires_ticker: bool,"
  lines << "    pub description: &'static str,"
  lines << "}"
  lines << ""
  lines << "#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]"
  lines << "#[repr(u8)]"
  lines << "#[cfg_attr("
  lines << "    feature = \"python\","
  lines << "    pyo3::pyclass(module = \"nautilus_trader.adapters.unusual_whales\", eq, from_py_object)"
  lines << ")]"
  lines << "#[cfg_attr("
  lines << "    feature = \"python\","
  lines << "    pyo3_stub_gen::derive::gen_stub_pyclass_enum("
  lines << "        module = \"nautilus_trader.adapters.unusual_whales\""
  lines << "    )"
  lines << ")]"
  lines << "pub enum UnusualWhalesChannelForm {"
  channel_variants.each_key { |variant| lines << "    #{variant}," }
  lines << "}"
  lines << ""
  lines << "pub static CHANNEL_FORMS: &[&str] = &["
  channel_variants.each_value do |channel|
    lines << "    #{rust_string(channel.fetch('form'))},"
  end
  lines << "];"
  lines << ""
  lines << "impl UnusualWhalesChannelForm {"
  lines << "    #[must_use]"
  lines << "    pub fn form(self) -> &'static str {"
  lines << "        CHANNEL_FORMS[self as usize]"
  lines << "    }"
  lines << "}"
  lines << ""
  lines << "#[rustfmt::skip]"
  lines << "pub static OPERATIONS: &[OperationSpec] = &["
  operations.each do |operation|
    lines << "    OperationSpec {"
    lines << "        operation_id: #{rust_string(operation.fetch('operation_id'))},"
    lines << "        method: #{rust_string(operation.fetch('method'))},"
    lines << "        path: #{rust_string(operation.fetch('path'))},"
    classification = operation.fetch("classification") == "read" ? "Read" : "AccountMutation"
    lines << "        classification: OperationClassification::#{classification},"
    lines << "        parameters: &["
    operation.fetch("parameters").each do |parameter|
      lines << "            ParameterSpec {"
      lines << "                name: #{rust_string(parameter.fetch('name'))},"
      lines << "                location: #{rust_string(parameter.fetch('in'))},"
      lines << "                required: #{parameter.fetch('required')},"
      lines << "                style: #{rust_string(parameter.fetch('style'))},"
      lines << "                explode: #{parameter.fetch('explode')},"
      lines << "                allow_reserved: #{parameter.fetch('allow_reserved')},"
      lines << "                schema_json: #{rust_string(canonical_json(parameter.fetch('schema')))},"
      lines << "                resolved_schema_json: #{rust_string(canonical_json(parameter.fetch('resolved_schema')))},"
      lines << "            },"
    end
    lines << "        ],"
    request_body = operation.fetch("request_body")
    lines << "        request_body_json: #{request_body ? "Some(#{rust_string(canonical_json(request_body))})" : 'None'},"
    lines << "        responses_json: #{rust_string(canonical_json(operation.fetch('responses')))},"
    refs = operation.fetch("response_schema_refs").map { |reference| rust_string(reference) }.join(", ")
    lines << "        response_schema_refs: &[#{refs}],"
    lines << "        security_json: #{rust_string(canonical_json(operation.fetch('security')))},"
    lines << "    },"
  end
  lines << "];"
  lines << ""
  lines << "#[rustfmt::skip]"
  lines << "pub static CHANNELS: &[ChannelSpec] = &["
  channels.each do |channel|
    form = channel.fetch("form")
    lines << "    ChannelSpec {"
    lines << "        form: #{rust_string(form)},"
    lines << "        prefix: #{rust_string(form.delete_suffix(':TICKER'))},"
    lines << "        requires_ticker: #{form.end_with?(':TICKER')},"
    lines << "        description: #{rust_string(channel.fetch('description'))},"
    lines << "    },"
  end
  lines << "];"
  lines << ""
  lines << "#[must_use]"
  lines << "pub fn find_operation(operation_id: &str) -> Option<&'static OperationSpec> {"
  lines << "    OPERATIONS"
  lines << "        .binary_search_by_key(&operation_id, |operation| operation.operation_id)"
  lines << "        .ok()"
  lines << "        .map(|index| &OPERATIONS[index])"
  lines << "}"
  lines << ""
  lines.join("\n")
end

def write_or_check(path, content, check)
  if check
    abort("Generated artifact differs: #{path}") unless path.file? && path.binread == content.b
  else
    FileUtils.mkdir_p(path.dirname)
    Tempfile.create([path.basename.to_s, ".tmp"], path.dirname.to_s) do |temp|
      temp.binmode
      temp.write(content)
      temp.flush
      File.rename(temp.path, path)
    end
  end
end

options = parse_options
script_dir = Pathname(__dir__)
crate_dir = script_dir.parent
source_path = crate_dir / "resources" / "openapi.yaml"
normalized_path = crate_dir / "generated" / "openapi.json"
catalog_path = crate_dir / "generated" / "catalog.json"
metadata_path = crate_dir / "generated" / "metadata.json"
rust_path = crate_dir / "src" / "generated.rs"

source = options.source ? Pathname(options.source).binread : fetch_source
document = parse_yaml(source)
abort("OpenAPI document must be an object") unless document.is_a?(Hash)
assert_exact_keys!(document, TOP_LEVEL_KEYS, "OpenAPI document")
abort("OpenAPI components must be an object") unless document["components"].is_a?(Hash)
assert_exact_keys!(document.fetch("components"), COMPONENT_KEYS, "OpenAPI components")
validate_document_metadata!(document)
document.fetch("components").fetch("schemas", {}).each do |name, schema|
  validate_schema!(schema, document, "components.schemas.#{name}")
end

operations = collect_operations(document)
channels = extract_channels(document)
method_counts = operations.group_by { |operation| operation.fetch("method") }.transform_values(&:length)
metadata = {
  "source_url" => SOURCE_URL,
  "source_sha256" => Digest::SHA256.hexdigest(source),
  "path_count" => document.fetch("paths").length,
  "operation_count" => operations.length,
  "method_counts" => method_counts,
  "channel_form_count" => channels.length,
}
catalog = {
  "metadata" => metadata,
  "operations" => operations,
  "channels" => channels,
}

write_or_check(source_path, source, options.check) unless options.source
write_or_check(normalized_path, JSON.pretty_generate(canonical(document)) + "\n", options.check)
write_or_check(catalog_path, JSON.pretty_generate(canonical(catalog)) + "\n", options.check)
write_or_check(metadata_path, JSON.pretty_generate(canonical(metadata)) + "\n", options.check)
write_or_check(rust_path, render_rust(metadata, operations, channels), options.check)

puts JSON.generate(metadata)
