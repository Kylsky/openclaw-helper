module.exports = [
  {
    ignores: ["node_modules/**", "release/**", "dist/**", "src-tauri/target/**", "src-tauri/gen/**"]
  },
  {
    files: ["src/**/*.js"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "script",
      globals: {
        require: "readonly",
        module: "readonly",
        __dirname: "readonly",
        process: "readonly",
        Buffer: "readonly",
        setTimeout: "readonly",
        clearTimeout: "readonly",
        setInterval: "readonly",
        clearInterval: "readonly"
      }
    },
    rules: {
      "no-unused-vars": ["warn", { argsIgnorePattern: "^_" }]
    }
  },
  {
    files: ["src/renderer/**/*.js"],
    languageOptions: {
      globals: {
        window: "readonly",
        document: "readonly",
        installer: "readonly"
      }
    }
  }
];
