# Gemini Image Generation

## meta
- kind: worker
- profile: proxy-google/gemini-3.1-flash-image-preview
- workdir: /tmp/cteno-test-imggen
- max-turns: 5

## setup
```bash
mkdir -p /tmp/cteno-test-imggen
```

## cases

### [pass] basic image generation
- **message**: "Draw a cat sitting on a beach at sunset"
- **expect**: Agent response contains at least one generated image (PNG/JPEG file saved to workdir), plus descriptive text about the image
- **anti-pattern**: Pure text response with no image / error about missing tools / calls non-existent image_generation tool
- **severity**: high

### [pass] mixed text and image response
- **message**: "Generate a wallpaper of a mountain landscape and describe what you created in detail"
- **expect**: Response includes both a generated image AND descriptive text (not just one or the other)
- **anti-pattern**: Only image with no text / only text with no image / image saved but not displayed
- **severity**: medium

### [pass] vague image request
- **message**: "I need a logo"
- **expect**: Agent generates some kind of logo image despite the vague request, rather than refusing or asking for too many clarifications
- **anti-pattern**: Refuses to generate / asks multiple clarification questions without generating anything / calls shell to install image tools
- **severity**: medium
