import { Nav } from '../components/Nav'
import { Hero } from '../components/Hero'
import { Features } from '../components/Features'
import { Modes } from '../components/Modes'
import { Providers } from '../components/Providers'
import { GetHosted } from '../components/GetHosted'
import { QuickStart } from '../components/QuickStart'
import { Footer } from '../components/Footer'

export function LandingPage() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <Features />
        <Modes />
        <Providers />
        <GetHosted />
        <QuickStart />
      </main>
      <Footer />
    </>
  )
}
