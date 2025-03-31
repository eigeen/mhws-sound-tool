# MHWS Sound Tool

[中文说明](docs/README_zh-CN.md)

## Introduction

This is a simple tool for extracting and editing Wwise Sound Bank (BNK) and PCK files.

Features: Compared to GUI tools like [RingingBloom](https://github.com/Silvris/RingingBloom), `MHWS Sound Tool` is simpler and more user-friendly for advanced users. Furthermore, since it unpacks wem files into project folders, it is better suited for writing scripts to perform batch operations or for use in conjunction with other tools for processing, rather than being limited to the GUI of specific tools.

## Download

Download from [Releases](https://github.com/eigeen/mhws-sound-tool/releases)

## Usage

### Extracting files and generate project folder

Supported file types:

- Wwise BNK file (designed for v145, but should work with other versions)
- Wwise PCK file

```
mhws-sound-tool.exe <input_file> [<input_file>...]
```

Drag and drop files onto the executable is easier to use.

![Drag and drop files](docs/img/drag-and-drop-file.png)

After that, you can see the generated `project` folder(s) near the input file(s).

![generated project folder](docs/img/generated-project-folder.png)

Project folder structure like this:

```
<.project>
├── [000]123456.wem
├── [001]2345678.wem
├── ...
├── project.json
├── bnk.json
```

Wem naming rules:

- `[000]` is the order index of the sound in the bnk file. The index is not important, just affect the order of the sounds inside the bnk file.
- `123456` is the unique ID of the sound file.
- The index is not important, duplications or random numbers are allowed. But you should keep the wem file name structure as `[number]number.wem`, so that the tool can recognize the file.
- The game find the wem by the unique ID, so the ID should be correct.

### Packaging project folder into target file

The entire folder should be seen as a project for `MHWS Sound Tool`. You should import the project folder, instead of individual files inside it.

![Drag and drop project folder](docs/img/drag-and-drop-project-folder.png)

Then you can see the generated target file(s) near the project folder `<original_file_name>.new`.
