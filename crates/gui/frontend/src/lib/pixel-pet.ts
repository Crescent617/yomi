import type { PetMood } from "./api";
import type PhaserType from "phaser";

export type PixelPetMood = PetMood;

export interface PixelPetController {
  destroy(): void;
  resetGaze(): void;
  setMood(mood: PixelPetMood): void;
}

const GAME_WIDTH = 152;
const GAME_HEIGHT = 112;
const PIXEL_SCALE = 3;

export async function mountPixelPet(
  parent: HTMLElement,
  initialMood: PixelPetMood,
): Promise<PixelPetController> {
  const { default: Phaser } = await import("phaser");
  let scene: PixelPetScene | undefined;
  let desiredMood = initialMood;

  const game = new Phaser.Game({
    type: Phaser.CANVAS,
    parent,
    width: GAME_WIDTH,
    height: GAME_HEIGHT,
    transparent: true,
    antialias: false,
    pixelArt: true,
    roundPixels: true,
    banner: false,
    audio: { noAudio: true },
    scale: {
      mode: Phaser.Scale.NONE,
      width: GAME_WIDTH,
      height: GAME_HEIGHT,
    },
    render: {
      antialias: false,
      antialiasGL: false,
      pixelArt: true,
      roundPixels: true,
      transparent: true,
      powerPreference: "low-power",
    },
    scene: {
      create(this: PhaserType.Scene) {
        scene = new PixelPetScene(Phaser, this, desiredMood);
      },
      update(_time: number, delta: number) {
        scene?.update(delta);
      },
    },
    callbacks: {
      postBoot(bootedGame) {
        const canvas = bootedGame.canvas;
        canvas.setAttribute("aria-hidden", "true");
      },
    },
  });

  return {
    setMood(mood) {
      desiredMood = mood;
      scene?.setMood(mood);
    },
    resetGaze() {
      scene?.resetGaze();
    },
    destroy() {
      scene?.destroy();
      game.destroy(true);
    },
  };
}

class PixelPetScene {
  private readonly root: PhaserType.GameObjects.Container;
  private readonly body: PhaserType.GameObjects.Graphics;
  private readonly face: PhaserType.GameObjects.Graphics;
  private readonly accent: PhaserType.GameObjects.Graphics;
  private readonly particles: PhaserType.GameObjects.Graphics;
  private readonly shadow: PhaserType.GameObjects.Ellipse;
  private mood: PixelPetMood;
  private elapsed = 0;
  private blinkAt = 1850 + Math.random() * 1300;
  private blinkRemaining = 0;
  private pointerX = GAME_WIDTH / 2;
  private pointerY = GAME_HEIGHT / 2;
  private pointerTracking = false;
  private idleGazeX = 0;
  private idleGazeY = 0;
  private idleGazeUntil = 0;
  private nextIdleGazeAt = this.randomIdleGazeDelay();
  private readonly handlePointerMove = (pointer: PhaserType.Input.Pointer) => {
    this.pointerTracking = true;
    this.pointerX = pointer.x;
    this.pointerY = pointer.y;
  };

  constructor(
    private readonly Phaser: typeof PhaserType,
    private readonly scene: PhaserType.Scene,
    initialMood: PixelPetMood,
  ) {
    this.mood = initialMood;
    this.shadow = scene.add.ellipse(77, 98, 62, 10, 0x15152c, 0.28);
    this.root = scene.add.container(76, 55);
    this.body = scene.add.graphics();
    this.face = scene.add.graphics();
    this.accent = scene.add.graphics();
    this.particles = scene.add.graphics();
    this.root.add([this.body, this.face, this.accent]);
    this.root.setScale(PIXEL_SCALE);
    this.drawBody();
    this.drawFrame();
    if (initialMood === "working") this.playWakeMotion();

    scene.input.on("pointermove", this.handlePointerMove);
  }

  resetGaze() {
    this.pointerTracking = false;
    this.pointerX = GAME_WIDTH / 2;
    this.pointerY = GAME_HEIGHT / 2;
    this.resetIdleGaze();
  }

  setMood(mood: PixelPetMood) {
    if (this.mood === mood) return;
    const wakingUp = this.mood === "sleepy" && mood !== "sleepy";
    this.mood = mood;
    this.elapsed = 0;
    this.resetIdleGaze();
    this.drawBody();
    this.drawFrame();
    this.scene.tweens.killTweensOf(this.root);
    if (mood === "working" || wakingUp) {
      this.playWakeMotion();
    } else if (mood === "happy") {
      this.scene.tweens.add({
        targets: this.root,
        y: 39,
        duration: 170,
        yoyo: true,
        repeat: 1,
        ease: "Quad.Out",
      });
    } else if (mood === "alert" || mood === "worried") {
      this.scene.tweens.add({
        targets: this.root,
        x: { from: 73, to: 79 },
        duration: 70,
        yoyo: true,
        repeat: 3,
      });
    }
  }

  update(delta: number) {
    this.elapsed += delta;
    this.updateIdleGaze();
    this.blinkAt -= delta;
    if (this.blinkAt <= 0 && this.blinkRemaining <= 0) {
      this.blinkRemaining = 130;
      this.blinkAt = 2200 + Math.random() * 2400;
    }
    if (this.blinkRemaining > 0) this.blinkRemaining -= delta;

    const t = this.elapsed / 1000;
    const motionRate = this.mood === "sleepy" ? 1.15 : 3.2;
    const baseY = this.mood === "happy" ? 55 : 57;
    const amplitude =
      this.mood === "working" ? 2.2 : this.mood === "sleepy" ? 0.7 : 1.25;
    if (!this.scene.tweens.isTweening(this.root)) {
      this.root.y = Math.round(baseY + Math.sin(t * motionRate) * amplitude);
      this.root.x = 76;
    }
    this.shadow.setScale(1 - Math.sin(t * motionRate) * 0.035, 1);
    this.shadow.setAlpha(0.25 - Math.sin(t * motionRate) * 0.025);
    this.drawBody();
    this.drawFrame();
  }

  destroy() {
    this.scene.input.off("pointermove", this.handlePointerMove);
    this.scene.tweens.killTweensOf(this.root);
  }

  private resetIdleGaze() {
    this.idleGazeX = 0;
    this.idleGazeY = 0;
    this.idleGazeUntil = 0;
    this.nextIdleGazeAt = this.elapsed + this.randomIdleGazeDelay();
  }

  private updateIdleGaze() {
    if (this.mood !== "idle" || this.pointerTracking) return;

    if (this.idleGazeUntil > 0) {
      if (this.elapsed < this.idleGazeUntil) return;
      this.idleGazeX = 0;
      this.idleGazeY = 0;
      this.idleGazeUntil = 0;
      this.nextIdleGazeAt = this.elapsed + this.randomIdleGazeDelay();
      return;
    }
    if (this.elapsed < this.nextIdleGazeAt) return;

    const directions = [
      [-1, 0],
      [1, 0],
      [-1, -1],
      [0, -1],
      [1, -1],
      [-1, 1],
      [1, 1],
    ] as const;
    const [x, y] = directions[Math.floor(Math.random() * directions.length)];
    this.idleGazeX = x;
    this.idleGazeY = y;
    this.idleGazeUntil = this.elapsed + 500 + Math.random() * 850;
  }

  private randomIdleGazeDelay() {
    return 3_000 + Math.random() * 6_000;
  }

  private playWakeMotion() {
    this.scene.tweens.add({
      targets: this.root,
      x: { from: 72, to: 80 },
      y: { from: 57, to: 49 },
      duration: 140,
      yoyo: true,
      repeat: 2,
      ease: "Quad.Out",
    });
  }

  private drawBody() {
    const g = this.body;
    g.clear();

    const workingEarTwitch =
      this.mood === "working" && Math.floor(this.elapsed / 120) % 24 === 0;
    const leftEarLift =
      this.mood === "alert" || workingEarTwitch
        ? -2
        : this.mood === "sleepy"
          ? 2
          : 0;
    const rightEarLift =
      this.mood === "alert" || this.mood === "curious" || workingEarTwitch
        ? -2
        : this.mood === "sleepy"
          ? 2
          : 0;

    // Soft aurora silhouette behind the pet.
    g.fillStyle(0x6d5bd0, 0.22);
    g.fillRect(-15, -12, 30, 25);
    g.fillRect(-17, -8, 34, 17);

    this.drawTail(g);

    // Ears react to attention while retaining the original silhouette.
    g.fillStyle(0x24213f, 1);
    g.fillRect(-13, -13 + leftEarLift, 7, 8);
    g.fillRect(6, -13 + rightEarLift, 7, 8);
    g.fillRect(-15, -9, 30, 17);
    g.fillRect(-13, 8, 26, 7);
    g.fillRect(-10, 15, 7, 3);
    g.fillRect(3, 15, 7, 3);

    // Main lavender fur.
    g.fillStyle(0x7868d7, 1);
    g.fillRect(-11, -10 + leftEarLift, 5, 7);
    g.fillRect(6, -10 + rightEarLift, 5, 7);
    g.fillRect(-12, -7, 24, 17);
    g.fillRect(-10, 10, 20, 4);

    // Face mask and highlights.
    g.fillStyle(0xeeeaff, 1);
    g.fillRect(-9, -4, 18, 11);
    g.fillRect(-7, 7, 14, 3);
    g.fillStyle(0xc6bcff, 1);
    g.fillRect(-10, -7, 5, 3);
    g.fillRect(5, -7, 5, 3);
    g.fillRect(-11, 9, 3, 3);
    g.fillStyle(0x9b8bed, 1);
    g.fillRect(-9, -11 + leftEarLift, 2, 4);
    g.fillRect(7, -11 + rightEarLift, 2, 4);
    g.fillRect(-12, 5, 2, 4);

    this.drawPaws(g);

    // Tiny scarf / terminal ribbon with dark lamp recesses.
    g.fillStyle(0x24213f, 1);
    g.fillRect(-7, 12, 14, 4);
    g.fillStyle(0x15152c, 1);
    g.fillRect(-5, 13, 2, 2);
    g.fillRect(-1, 13, 2, 2);
    g.fillRect(3, 13, 2, 2);
  }

  private drawTail(g: PhaserType.GameObjects.Graphics) {
    const workingSway =
      this.mood === "working" ? Math.floor(this.elapsed / 240) % 2 : 0;
    g.fillStyle(0x24213f, 1);
    if (this.mood === "sleepy") {
      g.fillRect(10, 10, 7, 5);
      g.fillRect(14, 7, 4, 5);
      g.fillStyle(0x7868d7, 1);
      g.fillRect(11, 11, 5, 3);
      g.fillRect(15, 8, 2, 3);
      return;
    }

    const tailLift =
      this.mood === "happy" || this.mood === "alert" ? -2 : workingSway;
    g.fillRect(11, 7 + tailLift, 5, 8);
    g.fillRect(14, 4 + tailLift, 5, 5);
    g.fillStyle(0x7868d7, 1);
    g.fillRect(12, 8 + tailLift, 3, 6);
    g.fillRect(15, 5 + tailLift, 3, 3);
  }

  private drawPaws(g: PhaserType.GameObjects.Graphics) {
    const workingStep = Math.floor(this.elapsed / 320) % 2;
    const tucked = this.mood === "sleepy";
    const leftX = tucked ? -6 : -11;
    const rightX = tucked ? 3 : 8;
    const leftY = this.mood === "working" ? 7 - workingStep : 7;
    const rightY = this.mood === "working" ? 6 + workingStep : 7;

    g.fillStyle(0x24213f, 1);
    g.fillRect(leftX, leftY, 3, 4);
    g.fillRect(rightX, rightY, 3, 4);
    g.fillStyle(0x9b8bed, 1);
    g.fillRect(leftX + 1, leftY, 2, 3);
    g.fillRect(rightX, rightY, 2, 3);
  }

  private drawFrame() {
    const g = this.face;
    const accent = this.accent;
    const particles = this.particles;
    g.clear();
    accent.clear();
    particles.clear();

    const lookX = this.Phaser.Math.Clamp(
      (this.pointerX - GAME_WIDTH / 2) / 65,
      -1,
      1,
    );
    const lookY = this.Phaser.Math.Clamp(
      (this.pointerY - GAME_HEIGHT / 2) / 50,
      -1,
      1,
    );
    const pointerEyeX = Math.round(lookX);
    const pointerEyeY = Math.round(lookY);
    const eyeX =
      this.mood === "idle" && !this.pointerTracking
        ? this.idleGazeX
        : pointerEyeX;
    const eyeY =
      this.mood === "idle" && !this.pointerTracking
        ? this.idleGazeY
        : pointerEyeY;
    const workingPhase = Math.floor(this.elapsed / 1_400) % 4;
    const wakePhase =
      this.mood === "working" && this.elapsed < 1_200
        ? Math.floor(this.elapsed / 300)
        : -1;
    const blinking = this.blinkRemaining > 0;

    // Sleepy is a terminal face state: do not let blink, gaze, or working
    // animation logic participate in its eye drawing.
    g.fillStyle(0xeeeaff, 1);
    g.fillRect(-8, -3, 16, 8);
    g.fillStyle(0x24213f, 1);
    if (this.mood === "sleepy") {
      this.drawSleepyFace(g);
    } else if (this.mood === "happy") {
      // Open sparkling eyes make completion distinct from sleepy closed eyes.
      g.fillRect(-7, -1, 3, 4);
      g.fillRect(4, -1, 3, 4);
      g.fillStyle(0xffffff, 1);
      g.fillRect(-6, -1, 1, 1);
      g.fillRect(5, -1, 1, 1);
    } else if (wakePhase === 0) {
      // Start closed, then pop open to make waking up unmistakable.
      g.fillRect(-7, 1, 4, 1);
      g.fillRect(3, 1, 4, 1);
    } else if (wakePhase === 1 || wakePhase === 2) {
      g.fillRect(-7, -1, 3, 4);
      g.fillRect(4, -1, 3, 4);
      g.fillStyle(0xffffff, 1);
      g.fillRect(-6, -1, 1, 1);
      g.fillRect(5, -1, 1, 1);
    } else if (blinking) {
      g.fillRect(-7, 1 + eyeY, 4, 1);
      g.fillRect(3, 1 + eyeY, 4, 1);
    } else if (this.mood === "working" && workingPhase === 1) {
      g.fillRect(-7, 1, 4, 1);
      g.fillRect(3, 1, 4, 1);
    } else {
      const workingEyeX =
        this.mood === "working"
          ? workingPhase === 0
            ? -1
            : workingPhase - 2
          : eyeX;
      g.fillRect(-6 + workingEyeX, 0 + eyeY, 2, 3);
      g.fillRect(4 + workingEyeX, 0 + eyeY, 2, 3);
      g.fillStyle(0xffffff, 1);
      g.fillRect(-5 + workingEyeX, 0 + eyeY, 1, 1);
      g.fillRect(5 + workingEyeX, 0 + eyeY, 1, 1);
    }

    g.fillStyle(0xff9ab3, this.mood === "alert" ? 0.5 : 0.85);
    g.fillRect(-10, 4, 2, 1);
    g.fillRect(8, 4, 2, 1);

    g.fillStyle(0x24213f, 1);
    if (this.mood === "happy") {
      // Wide U-shaped smile for a completed mission.
      g.fillRect(-3, 4, 1, 1);
      g.fillRect(2, 4, 1, 1);
      g.fillRect(-2, 5, 4, 1);
    } else if (this.mood === "worried" || this.mood === "alert") {
      g.fillRect(-2, 5, 4, 1);
      g.fillRect(-1, 4, 2, 1);
    } else if (this.mood === "curious") {
      g.fillRect(-1, 4, 2, 2);
    } else if (this.mood === "sleepy") {
      // A tiny shallow smile matches the soft curve of the sleeping eyes.
      g.fillRect(-1, 4, 3, 1);
      g.fillRect(0, 5, 1, 1);
    } else if (wakePhase === 1 || wakePhase === 2) {
      g.fillRect(-3, 4, 1, 1);
      g.fillRect(2, 4, 1, 1);
      g.fillRect(-2, 5, 4, 1);
    } else if (this.mood === "working" && workingPhase === 3) {
      g.fillRect(-2, 4, 4, 1);
      g.fillRect(-1, 5, 2, 1);
    } else {
      g.fillRect(-1, 4, 2, 1);
    }

    this.drawMoodAccent(accent, particles);
  }

  private drawSleepyFace(g: PhaserType.GameObjects.Graphics) {
    // Slightly curved eyelids keep the sleeping expression soft and peaceful.
    g.fillRect(-7, 1, 1, 1);
    g.fillRect(-6, 2, 3, 1);
    g.fillRect(-3, 1, 1, 1);
    g.fillRect(3, 1, 1, 1);
    g.fillRect(4, 2, 3, 1);
    g.fillRect(7, 1, 1, 1);
  }

  private drawMoodAccent(
    accent: PhaserType.GameObjects.Graphics,
    particles: PhaserType.GameObjects.Graphics,
  ) {
    const phase = Math.floor(this.elapsed / 230) % 3;
    this.drawStatusLamps(accent);

    if (this.mood === "working") {
      if (this.elapsed < 1_200) {
        particles.fillStyle(0xffcf70, 1);
        particles.fillRect(31, 36, 4, 4);
        particles.fillRect(117, 31, 3, 3);
      }
      accent.fillStyle(0x50e3c2, 1);
      accent.fillRect(-14 + phase * 2, 12, 2, 1);
      particles.fillStyle(0x50e3c2, 0.85);
      particles.fillRect(31 + phase * 4, 70 - phase * 4, 4, 4);
      particles.fillStyle(0xc6bcff, 0.75);
      particles.fillRect(113 - phase * 3, 29 + phase * 3, 3, 3);
    } else if (this.mood === "happy") {
      particles.fillStyle(0xffcf70, 1);
      particles.fillRect(26, 40, 4, 4);
      particles.fillRect(119, 48, 3, 3);
      particles.fillStyle(0xff719b, 1);
      particles.fillRect(34, 31, 3, 3);
      particles.fillRect(112, 35, 4, 4);
    } else if (this.mood === "curious") {
      particles.fillStyle(0x88d9ff, 1);
      particles.fillRect(115, 29, 5, 5);
      particles.fillRect(117, 23, 3, 3);
      particles.fillRect(117, 36, 3, 3);
    } else if (this.mood === "alert") {
      particles.fillStyle(0xffcf70, 1);
      particles.fillRect(116, 25, 4, 14);
      particles.fillRect(116, 43, 4, 4);
    } else if (this.mood === "worried") {
      particles.fillStyle(0x88d9ff, 0.9);
      particles.fillRect(115, 32 + phase * 3, 4, 7);
    } else if (this.mood === "sleepy") {
      const zPhase = Math.floor(this.elapsed / 520) % 3;
      particles.fillStyle(0xc6bcff, 0.65 + zPhase * 0.1);
      this.drawZParticle(particles, 108 + zPhase * 2, 42 - zPhase * 5, 5);
      this.drawZParticle(particles, 114 + zPhase * 2, 33 - zPhase * 5, 6);
      this.drawZParticle(particles, 121 + zPhase * 2, 23 - zPhase * 5, 7);
    } else {
      particles.fillStyle(0xc6bcff, 0.45);
      particles.fillRect(27, 58 + phase, 3, 3);
      particles.fillRect(121, 50 - phase, 3, 3);
    }
  }

  private drawStatusLamps(accent: PhaserType.GameObjects.Graphics) {
    const lampX = [-5, -1, 3] as const;
    const drawLamp = (index: number, color: number, alpha: number) => {
      accent.fillStyle(color, alpha);
      accent.fillRect(lampX[index], 13, 2, 2);
    };

    if (this.mood === "sleepy") {
      const fade = Math.max(0, 0.4 - this.elapsed / 1_800);
      for (let index = 0; index < lampX.length; index += 1) {
        drawLamp(index, 0x50e3c2, fade);
      }
      return;
    }

    if (this.mood === "working") {
      const activeLamp = Math.floor(this.elapsed / 320) % 3;
      for (let index = 0; index < lampX.length; index += 1) {
        drawLamp(index, 0x50e3c2, index === activeLamp ? 1 : 0.16);
      }
      return;
    }

    if (this.mood === "curious") {
      const activeLamp = Math.floor(this.elapsed / 620) % 3;
      for (let index = 0; index < lampX.length; index += 1) {
        drawLamp(index, 0x88d9ff, index === activeLamp ? 0.85 : 0.24);
      }
      return;
    }

    if (this.mood === "alert") {
      const pulse = 0.5 + (Math.sin(this.elapsed / 280) + 1) * 0.25;
      drawLamp(0, 0xffcf70, pulse * 0.7);
      drawLamp(1, 0xffcf70, pulse);
      drawLamp(2, 0xffcf70, pulse * 0.7);
      return;
    }

    if (this.mood === "happy") {
      const flash = this.elapsed < 700 ? 1 : 0.45;
      for (let index = 0; index < lampX.length; index += 1) {
        drawLamp(index, 0x50e3c2, flash);
      }
      return;
    }

    if (this.mood === "worried") {
      const pulse = 0.35 + (Math.sin(this.elapsed / 520) + 1) * 0.3;
      drawLamp(2, 0xff719b, pulse);
      return;
    }

    for (let index = 0; index < lampX.length; index += 1) {
      drawLamp(index, 0x50e3c2, 0.28);
    }
  }

  private drawZParticle(
    graphics: PhaserType.GameObjects.Graphics,
    x: number,
    y: number,
    width: number,
  ) {
    graphics.fillRect(x, y, width, 1);
    graphics.fillRect(x + width - 2, y + 1, 2, 1);
    graphics.fillRect(x + 1, y + 2, 2, 1);
    graphics.fillRect(x, y + 3, width, 1);
  }
}
